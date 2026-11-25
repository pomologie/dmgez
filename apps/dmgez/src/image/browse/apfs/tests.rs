use std::fs::{self, File};

use dmgez_udif::{
	Cancellation, Error as ImageError,
	writer::{Profile, WriteOptions, write_image},
};
use dmgez_vfs::{
	PartitionId, Scheme,
	volume::{NodeKind, RawName},
};

use crate::{
	image::{
		ImageTab,
		browse::{Error, Filesystem, Storage, content, test_support::activate},
	},
	inspector::Action,
};

use super::*;

#[path = "../../../../../dmgez-cli/tests/support/native_apfs.rs"]
mod fixture;

const VOLUME_NAME: &str = "Pomologie APFS Hard Links";
const DOCS: &str = "Docs";
const ALIAS: &str = "alias.bin";
const DATA: &str = "APFS shared data fork\n";
const RESOURCE: &str = "APFS shared resource fork\n";
const DATA_REPETITIONS: usize = 250;
const RESOURCE_REPETITIONS: usize = 200;

fn disk_image(fixture_name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
	let workspace = tempfile::tempdir().unwrap();
	let raw = workspace.path().join("native.raw");
	fixture::materialize(&raw, fixture_name, &[]);
	let dmg = workspace.path().join("native.dmg");
	write_image(
		File::open(raw).unwrap(),
		fixture::NATIVE_VOLUME_BYTES,
		&mut File::create(&dmg).unwrap(),
		&WriteOptions {
			profile: Profile::Udro,
			..WriteOptions::default()
		},
	)
	.unwrap();
	(workspace, dmg)
}

fn observe(tab: &ImageTab, request: Request) -> Observation {
	tab.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
}

#[test]
fn selects_the_native_apfs_checkpoint_and_browses_exact_files_through_files_view() {
	let (_workspace, dmg) = disk_image(fixture::HARD_LINK_FIXTURE);
	let mut tab = ImageTab::open(File::open(dmg).unwrap()).unwrap();
	assert!(matches!(activate(&mut tab, "Files"), Action::ShowFiles));
	tab.browser.showing = true;
	let Action::Browse(request @ Request::ApfsCatalog(VolumeId::WholeDisk)) =
		activate(&mut tab, "Check whole disk for APFS")
	else {
		panic!("expected the APFS discovery action");
	};
	let Observation::ApfsCatalog(catalog) = observe(&tab, request) else {
		panic!("expected the APFS volume list");
	};
	let [volume] = catalog.volumes.as_slice() else {
		panic!("expected the native fixture's only volume");
	};
	assert_eq!(volume.name.as_str(), VOLUME_NAME);
	assert_eq!(
		volume.checkpoint.0.to_string(),
		fixture::HARD_LINK_CHECKPOINT_XID
	);
	assert_eq!(
		volume.object_id.0.to_string(),
		fixture::HARD_LINK_VOLUME_OID
	);
	let observed_uuid = volume
		.uuid
		.iter()
		.map(|byte| format!("{byte:02X}"))
		.collect::<String>();
	assert_eq!(
		observed_uuid,
		fixture::HARD_LINK_VOLUME_UUID.replace('-', "")
	);
	let title = format!("Open {VOLUME_NAME} (object {})", volume.object_id.0);
	tab.browser.apfs = Some(catalog);
	let Action::Browse(request) = activate(&mut tab, &title) else {
		panic!("expected the APFS volume selection action");
	};
	let Observation::Page(root) = observe(&tab, request) else {
		panic!("expected the APFS root page");
	};
	assert_eq!(root.filesystem, Filesystem::Apfs);
	let mut selected = root.location.apfs.unwrap();
	assert!(tab.browser.show_page(root, None));
	let Action::Browse(request) = activate(&mut tab, DOCS) else {
		panic!("expected the native APFS folder action");
	};
	let Observation::Page(docs) = observe(&tab, request) else {
		panic!("expected the APFS folder page");
	};
	assert_eq!(docs.location.apfs, Some(selected));
	assert_eq!(docs.location.ancestor(0).apfs, Some(selected));
	let location = docs.location;
	for (query, expected) in [("alias", 1), ("ALIAS", 0)] {
		let Observation::Search(report) = observe(
			&tab,
			Request::Search {
				location: location.clone(),
				query: query.into(),
			},
		) else {
			panic!("expected bounded APFS search results");
		};
		assert_eq!(report.entries.len(), expected);
	}
	let Observation::Details(details) = observe(
		&tab,
		Request::Text {
			folder: location.clone(),
			name: RawName::Utf8(ALIAS.into()),
		},
	) else {
		panic!("expected the APFS hard-link details");
	};
	assert_eq!(details.metadata.kind, NodeKind::RegularFile);
	assert_eq!(details.metadata.hard_link.unwrap().count.get(), 3);
	#[cfg(feature = "hfsplus-read")]
	let Storage::Apfs(storage) = details.storage.as_ref() else {
		panic!("metadata should come from the selected APFS view");
	};
	#[cfg(not(feature = "hfsplus-read"))]
	let Storage::Apfs(storage) = details.storage.as_ref();
	let forks = storage.forks.as_ref().unwrap();
	assert!(
		matches!(forks.data, Ok(Some(bytes)) if bytes == (DATA.len() * DATA_REPETITIONS) as u64)
	);
	assert!(
		matches!(forks.resource, Ok(Some(bytes)) if bytes == (RESOURCE.len() * RESOURCE_REPETITIONS) as u64)
	);
	assert_eq!(
		details.text,
		Some(content::Preview::Text {
			value: DATA.repeat(DATA_REPETITIONS),
			extent: content::Extent::Complete,
		})
	);
	selected.uuid[0] ^= 1;
	let moved = Location {
		apfs: Some(selected),
		..location
	};
	assert!(matches!(
		tab.snapshot
			.browse(Request::first(moved), &Cancellation::default()),
		Err(Error::ApfsVolumeChanged)
	));
}

#[test]
fn apfs_refuses_a_foreign_window_cancelled_work_and_a_changed_held_source() {
	let (_workspace, dmg) = disk_image(fixture::SPARSE_FIXTURE);
	let tab = ImageTab::open(File::open(&dmg).unwrap()).unwrap();
	let foreign = VolumeId::Partition(PartitionId {
		scheme: Scheme::Gpt,
		slot: std::num::NonZeroU32::MIN,
	});
	assert!(matches!(
		tab.snapshot
			.browse(Request::ApfsCatalog(foreign), &Cancellation::default()),
		Err(Error::ApfsWindow(_))
	));
	let cancellation = Cancellation::default();
	cancellation.cancel();
	assert!(matches!(
		tab.snapshot
			.browse(Request::ApfsCatalog(VolumeId::WholeDisk), &cancellation),
		Err(Error::Image(ImageError::Cancelled))
	));
	fs::write(dmg, b"changed opened image").unwrap();
	assert!(matches!(
		tab.snapshot.browse(
			Request::ApfsCatalog(VolumeId::WholeDisk),
			&Cancellation::default()
		),
		Err(Error::Image(ImageError::SourceChanged))
	));
}
