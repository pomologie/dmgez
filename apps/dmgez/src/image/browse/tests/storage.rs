use dmgez_udif::Cancellation;
use dmgez_vfs::{VolumeId, volume::RawName};
use eframe::egui;

use super::{activate, fixture, hfs_storage, materialize_sparse, page, wrap};
use crate::{
	image::{
		ImageTab,
		browse::{Details, Location, Observation, Request},
	},
	inspector::{self, Action},
};

const HARD_LINKS: &str = "native-chained-hardlinks.bin";
const HARD_LINK_DATA: &[u8] = b"pomologie linked file\n";
const HARD_LINK_RESOURCE: &[u8] = b"pomologie resource\0fork\n";
const COMPRESSED: &str = "native-decmpfs-zlib.bin";
const COMPRESSED_RESOURCE_BYTES: u64 = 1_115;
const LOGICAL_RESOURCE_BYTES: u64 = (3 << 16) + 513;
const COMPRESSION_ATTRIBUTE: &str = "com.apple.decmpfs";

fn details(tab: &ImageTab, name: &str) -> Details {
	let observation = tab
		.snapshot
		.browse(
			Request::Details {
				folder: Location::root(VolumeId::WholeDisk),
				name: RawName::Utf16(name.encode_utf16().collect()),
			},
			&Cancellation::default(),
		)
		.unwrap();
	let Observation::Details(details) = observation else {
		panic!("expected item details for {name}");
	};
	details
}

#[test]
fn shows_native_backing_fork_lengths_for_visible_hard_links() {
	let tab = wrap(&materialize_sparse(&fixture(HARD_LINKS)));
	for name in ["one", "two"] {
		let details = details(&tab, name);
		assert!(details.metadata.hard_link.is_some());
		let forks = hfs_storage(&details).forks.as_ref().unwrap();
		assert!(matches!(forks.data, Ok(Some(bytes)) if bytes == HARD_LINK_DATA.len() as u64));
		assert!(
			matches!(forks.resource, Ok(Some(bytes)) if bytes == HARD_LINK_RESOURCE.len() as u64)
		);
	}
	assert!(hfs_storage(&details(&tab, "nested")).forks.is_none());
}

#[test]
fn shows_exact_attribute_names_and_resource_size_even_without_a_data_decoder() {
	let mut tab = wrap(&materialize_sparse(&fixture(COMPRESSED)));
	tab.browser.showing = true;
	let observed = page(&tab, Location::root(VolumeId::WholeDisk), 0);
	assert!(tab.browser.show_page(observed, None));
	let Action::Browse(request @ Request::Details { .. }) =
		activate(&mut tab, "Details for resource")
	else {
		panic!("expected the resource file details action");
	};
	let observation = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap();
	let Observation::Details(details) = observation else {
		panic!("expected item details from the selected row");
	};
	let forks = hfs_storage(&details).forks.as_ref().unwrap();
	assert!(matches!(
		forks.resource,
		Ok(Some(COMPRESSED_RESOURCE_BYTES))
	));
	match &forks.data {
		Ok(Some(bytes)) => assert_eq!(*bytes, LOGICAL_RESOURCE_BYTES),
		Err(error) => {
			let cause = std::error::Error::source(error).unwrap();
			assert_eq!(cause.to_string(), "decmpfs zlib support is disabled");
		}
		Ok(None) => panic!("expected a data fork or a decoder error"),
	}
	let names = hfs_storage(&details)
		.xattrs
		.as_ref()
		.unwrap()
		.as_ref()
		.unwrap();
	assert!(names.contains(&RawName::Utf16(
		COMPRESSION_ATTRIBUTE.encode_utf16().collect()
	)));

	tab.browser.details = Some(details);
	let context = egui::Context::default();
	context.enable_accesskit();
	let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::INITIAL_SIZE.into());
	let mut output = context.run_ui(
		egui::RawInput {
			screen_rect: Some(viewport),
			..Default::default()
		},
		|ui| assert!(inspector::show(ui, &mut tab, true).is_none()),
	);
	let update = output.platform_output.accesskit_update.take().unwrap();
	output.drop_without_applying_deltas();
	for label in [
		"Data fork",
		"Resource fork",
		"Extended attributes",
		COMPRESSION_ATTRIBUTE,
	] {
		assert!(
			update
				.nodes
				.iter()
				.any(|(_, node)| node.label() == Some(label) || node.value() == Some(label)),
			"item details should expose {label}"
		);
	}
}
