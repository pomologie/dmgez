use std::{
	fs::{self, File},
	num::NonZeroU32,
	path::PathBuf,
};

use dmgez_udif::writer::{Profile, WriteOptions, write_image};
use dmgez_vfs::{PartitionId, Scheme};
use eframe::egui::{self, accesskit};

use super::*;
use crate::{
	image::ImageTab,
	inspector::{self, Action},
};

mod storage;

const NATIVE_APM: &[u8] =
	include_bytes!("../../../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
const FIXTURES: &str = "DMGEZ_HFSPLUS_FIXTURES";
const CATALOG: &str = "native-catalog-volume.bin";
const HFSX: &str = "native-hfsx.bin";
const NATIVE_HFS_PARTITION: u32 = 2;
const NATIVE_METADATA_PARTITION: u32 = 1;
const NATIVE_CATALOG_ENTRIES: usize = 97;
const NATIVE_NESTED_PAYLOAD: &[u8] = b"catalog unicode payload\n";

fn fixture(name: &str) -> Vec<u8> {
	let root = std::env::var_os(FIXTURES).expect("desktop composition supplies HFS+ fixtures");
	fs::read(PathBuf::from(root).join(name)).unwrap()
}

fn wrap(raw: &[u8]) -> ImageTab {
	let mut image = tempfile::tempfile().unwrap();
	write_image(
		raw,
		raw.len() as u64,
		&mut image,
		&WriteOptions {
			profile: Profile::Udro,
			..WriteOptions::default()
		},
	)
	.unwrap();
	ImageTab::open(image).unwrap()
}

fn page(tab: &ImageTab, location: Location, offset: u64) -> Page {
	let observation = tab
		.snapshot
		.browse(Request::Page { location, offset }, &Cancellation::default())
		.unwrap();
	let Observation::Page(page) = observation else {
		panic!("expected a folder page");
	};
	page
}

fn partition(slot: u32) -> Location {
	Location::root(VolumeId::Partition(PartitionId {
		scheme: Scheme::Apple,
		slot: NonZeroU32::new(slot).unwrap(),
	}))
}

#[test]
fn browses_only_an_admitted_native_partition_data_window() {
	use std::io::Write as _;

	let mut image = tempfile::tempfile().unwrap();
	image.write_all(NATIVE_APM).unwrap();
	let tab = ImageTab::open(image).unwrap();
	let observed = page(&tab, partition(NATIVE_HFS_PARTITION), 0);
	assert_eq!(observed.filesystem, VolumeSignature::HfsPlus);
	assert_eq!(observed.location, partition(NATIVE_HFS_PARTITION));
	assert!(matches!(
		tab.snapshot.browse(
			Request::first(partition(NATIVE_METADATA_PARTITION)),
			&Cancellation::default()
		),
		Err(Error::Disk(dmgez_vfs::Error::NoDataWindow(_)))
	));
}

#[test]
fn native_catalog_pages_and_directory_components_retain_exact_stored_names() {
	let tab = wrap(&fixture(CATALOG));
	let root = Location::root(VolumeId::WholeDisk);
	let first = page(&tab, root.clone(), 0);
	assert_eq!(first.entries.len() as u64, PAGE_ENTRIES);
	assert_eq!(first.next, Some(PAGE_ENTRIES));
	let second = page(&tab, root.clone(), first.next.unwrap());
	assert_eq!(
		first.entries.len() + second.entries.len(),
		NATIVE_CATALOG_ENTRIES
	);
	assert!(second.next.is_none());

	let nested = first
		.entries
		.iter()
		.chain(&second.entries)
		.find(|entry| label(&entry.name) == "Nested")
		.expect("native catalog fixture contains Nested");
	assert_eq!(nested.kind, NodeKind::Directory);
	let folder = root.child(&nested.name).unwrap();
	assert_eq!(folder.ancestor(0), root);
	let child = page(&tab, folder.clone(), 0);
	assert_eq!(child.entries.len(), 1);
	let entry = &child.entries[0];
	let expected = "Re\u{301}sume\u{301}.txt";
	assert_eq!(
		entry.name,
		RawName::Utf16(expected.encode_utf16().collect())
	);
	let observation = tab
		.snapshot
		.browse(
			Request::Details {
				folder: folder.clone(),
				name: entry.name.clone(),
			},
			&Cancellation::default(),
		)
		.unwrap();
	let Observation::Details(details) = observation else {
		panic!("expected item details");
	};
	assert_eq!(details.name, entry.name);
	assert_eq!(details.metadata.kind, NodeKind::RegularFile);
	assert!(details.native_id.is_some());
	let forks = details.storage.forks.as_ref().unwrap();
	assert!(matches!(forks.data, Ok(Some(bytes)) if bytes == NATIVE_NESTED_PAYLOAD.len() as u64));
	assert!(matches!(forks.resource, Ok(Some(0))));
	assert!(matches!(&details.storage.xattrs, Ok(Some(names)) if names.is_empty()));
	let file = folder.child(&entry.name).unwrap();
	assert!(matches!(
		tab.snapshot
			.browse(Request::first(file), &Cancellation::default()),
		Err(Error::NotDirectory)
	));
	let alias = root
		.child(&RawName::Utf16("nested".encode_utf16().collect()))
		.unwrap();
	assert!(matches!(
		tab.snapshot
			.browse(Request::first(alias), &Cancellation::default()),
		Err(Error::EntryChanged)
	));
}

#[test]
fn hfsx_keeps_case_distinct_directories_and_file_names() {
	let fixture = fixture(HFSX);
	let tab = wrap(&materialize_sparse(&fixture));
	let root = page(&tab, Location::root(VolumeId::WholeDisk), 0);
	assert_eq!(root.filesystem, VolumeSignature::HfsX);
	let names: Vec<_> = root
		.entries
		.iter()
		.map(|entry| label(&entry.name))
		.collect();
	assert_eq!(names, ["Cases", "cases"]);
	let upper = page(&tab, root.location.child(&root.entries[0].name).unwrap(), 0);
	let lower = page(&tab, root.location.child(&root.entries[1].name).unwrap(), 0);
	let names: Vec<_> = upper
		.entries
		.iter()
		.map(|entry| label(&entry.name))
		.collect();
	assert!(names.iter().any(|name| name == "Case.txt"));
	assert!(names.iter().any(|name| name == "case.txt"));
	assert!(lower.entries.is_empty());
}

#[test]
fn rejects_a_changed_source_and_cancelled_lookups() {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().join("partitioned.dmg");
	fs::write(&path, NATIVE_APM).unwrap();
	let tab = ImageTab::open(File::open(&path).unwrap()).unwrap();
	let cancellation = Cancellation::default();
	cancellation.cancel();
	assert!(matches!(
		tab.snapshot.browse(
			Request::first(partition(NATIVE_HFS_PARTITION)),
			&cancellation
		),
		Err(Error::Image(ImageError::Cancelled))
	));
	fs::write(path, b"changed length").unwrap();
	assert!(matches!(
		tab.snapshot.browse(
			Request::first(partition(NATIVE_HFS_PARTITION)),
			&Cancellation::default()
		),
		Err(Error::Image(ImageError::SourceChanged))
	));
}

#[test]
fn accessible_file_controls_request_volume_pages_and_folder_navigation() {
	let mut tab = wrap(&fixture(CATALOG));
	assert!(matches!(activate(&mut tab, "Files"), Action::ShowFiles));
	tab.browser.showing = true;
	for control in ["Open whole-disk volume", "Next files", "Nested", "Up"] {
		let Action::Browse(request) = activate(&mut tab, control) else {
			panic!("expected browse request from {control}");
		};
		let Observation::Page(observed) = tab
			.snapshot
			.browse(request, &Cancellation::default())
			.unwrap()
		else {
			panic!("expected folder page from {control}");
		};
		let trail: Vec<_> = observed.location.path.iter().map(label).collect();
		if control == "Nested" {
			assert_eq!(trail, ["Nested"]);
		} else {
			assert!(trail.is_empty(), "{control}");
		}
		assert_eq!(observed.location.volume, VolumeId::WholeDisk);
		assert_eq!(
			observed.offset,
			if control == "Next files" {
				PAGE_ENTRIES
			} else {
				0
			}
		);
		tab.browser.page = Some(observed);
	}
}

#[cfg(feature = "actual-preview")]
#[test]
fn folder_preview_uses_the_admitted_location_and_a_bounded_texture() {
	use dmgez_preview::actual::StoreStatus;

	let mut tab = wrap(&fixture(CATALOG));
	let root = Location::root(VolumeId::WholeDisk);
	let nested = RawName::Utf16("Nested".encode_utf16().collect());
	let location = root.child(&nested).unwrap();
	tab.browser.showing = true;
	let selected = page(&tab, location.clone(), 0);
	tab.browser.show_page(selected);

	let Action::Browse(request @ Request::Preview { .. }) = activate(&mut tab, "Preview folder")
	else {
		panic!("expected a preview request from the current folder");
	};
	let Observation::Preview(prepared) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected an actual folder preview");
	};
	assert_eq!(prepared.location, location);
	let presented = prepared.present(&egui::Context::default()).unwrap();
	assert_eq!(presented.coverage.store, StoreStatus::Absent);
	assert_eq!(presented.coverage.fallback_positions, 1);
	assert!(presented.texture.size().iter().all(|&side| side > 1));
	tab.browser.preview = Some(presented);
	let root_page = page(&tab, root, 0);
	tab.browser.show_page(root_page);
	assert!(tab.browser.preview.is_none());

	assert!(matches!(
		tab.snapshot.browse(
			Request::Preview {
				location,
				max_side: 1
			},
			&Cancellation::default()
		),
		Err(Error::PreviewTextureLimit),
	));
}

#[test]
fn catalog_details_controls_share_a_column_on_separate_rows() {
	let mut tab = wrap(&fixture(CATALOG));
	tab.browser.showing = true;
	tab.browser.page = Some(page(&tab, Location::root(VolumeId::WholeDisk), 0));
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
	let controls: Vec<_> = output
		.platform_output
		.accesskit_update
		.take()
		.unwrap()
		.nodes
		.iter()
		.filter(|(_, node)| {
			node.label()
				.is_some_and(|label| label.starts_with("Details for "))
		})
		.filter_map(|(_, node)| node.bounds())
		.take(2)
		.collect();
	output.drop_without_applying_deltas();
	let [first, second] = controls.as_slice() else {
		panic!("expected details controls for two catalog entries");
	};
	assert!((first.x0 - second.x0).abs() < 1.0, "{first:?}, {second:?}");
	assert!((first.y0 - second.y0).abs() >= 1.0, "{first:?}, {second:?}");
}

fn activate(tab: &mut ImageTab, title: &str) -> Action {
	let context = egui::Context::default();
	context.enable_accesskit();
	let viewport = egui::Rect::from_min_size(
		egui::Pos2::ZERO,
		egui::vec2(crate::INITIAL_SIZE[0], 1_600.0),
	);
	let mut initial = context.run_ui(
		egui::RawInput {
			screen_rect: Some(viewport),
			..Default::default()
		},
		|ui| assert!(inspector::show(ui, tab, true).is_none()),
	);
	let update = initial.platform_output.accesskit_update.take().unwrap();
	initial.drop_without_applying_deltas();
	let target_node = update
		.nodes
		.into_iter()
		.find(|(_, node)| {
			node.label() == Some(title) && node.supports_action(accesskit::Action::Click)
		})
		.unwrap_or_else(|| panic!("missing reachable {title} control"))
		.0;
	let mut action = None;
	context
		.run_ui(
			egui::RawInput {
				screen_rect: Some(viewport),
				events: vec![egui::Event::AccessKitActionRequest(
					accesskit::ActionRequest {
						action: accesskit::Action::Click,
						target_tree: accesskit::TreeId::ROOT,
						target_node,
						data: None,
					},
				)],
				..Default::default()
			},
			|ui| {
				if let Some(emitted) = inspector::show(ui, tab, true) {
					action = Some(emitted);
				}
			},
		)
		.drop_without_applying_deltas();
	action.unwrap_or_else(|| panic!("no action emitted from {title}"))
}

// Encoding specified by the sibling reader's native-hfsx-case-sensitive.json ledger.
fn materialize_sparse(mut fixture: &[u8]) -> Vec<u8> {
	fn field<const WIDTH: usize>(fixture: &mut &[u8]) -> [u8; WIDTH] {
		let (field, tail) = fixture.split_first_chunk().unwrap();
		*fixture = tail;
		*field
	}
	let length = usize::try_from(u64::from_be_bytes(field(&mut fixture))).unwrap();
	let mut raw = vec![0; length];
	for _ in 0..u16::from_be_bytes(field(&mut fixture)) {
		let offset = usize::try_from(u64::from_be_bytes(field(&mut fixture))).unwrap();
		let length = u32::from_be_bytes(field(&mut fixture)) as usize;
		let (bytes, rest) = fixture.split_at(length);
		raw[offset..][..length].copy_from_slice(bytes);
		fixture = rest;
	}
	assert!(fixture.is_empty());
	raw
}
