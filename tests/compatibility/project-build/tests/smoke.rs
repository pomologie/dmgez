#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::{fs, num::NonZeroUsize, path::Path};

use dmgez_builder::{
	execution::{ExecuteError, ImageError},
	manifest,
	metadata::policy::{Field, LossReason},
	preflight::{OutputIntent, PreflightOptions, ReleaseIntent},
};
use dmgez_config::Project;
use dmgez_hfs_adapter_probe::hfsplus::{self, Error};
use dmgez_udif::reader::{FileSource, Image, Limits};
use hfsplus_format::{
	attributes::{InlineRecord, NATIVE_INLINE_MAX_VALUE_BYTES},
	btree::{Node, TreeHeader},
	catalog::{Record, Value},
	hard_links::{Inode, Stub, is_private_directory_key},
	volume::{Fork, Geometry, HEADER_BYTES, Header, PRIMARY_HEADER_OFFSET},
};
use rustix::fs::{XattrFlags, fsetxattr};

#[cfg(feature = "volume-icon")]
#[path = "smoke/volume_icon.rs"]
mod volume_icon;

const PROJECT_HEADER: &str =
	"schema_version = 1\n[image]\nname = 'Compose'\noutput = 'image.dmg'\n";
const IMAGE: &str = "image.dmg";
const INPUT_FILE: &str = "source.txt";
const INPUT: &[u8] = b"returned by the same inode";
const FREE_SPACE_BYTES: u64 = 64 * 1024;
const MARKER_NAME: &str = "user.pomologie";
const MARKER_BYTES: &[u8] = b"owned fixture";
const RESOURCE_NAME: &str = "com.apple.ResourceFork";
const RESOURCE_BYTES: &[u8] = b"fork\0fixture";
const TREE_HEADER_PREFIX: usize = 34;
const LINK_DIRECTORY: &str = "aliases";
const LINK_FILE: &str = "linked.txt";
const LINK_SOURCE: &str = "aliases/linked.txt";
const PRIMARY_DIRECTORY: &str = "selected";
const PRIMARY_SOURCE: &str = "selected/source.txt";
const INDEPENDENT_FILE: &str = "copy.txt";

fn options(output: OutputIntent) -> PreflightOptions {
	PreflightOptions {
		capabilities: hfsplus::capabilities(),
		output,
		release: ReleaseIntent::Unsigned,
		host_threads: NonZeroUsize::MIN,
	}
}

fn open_output(path: &Path) -> (Image<FileSource>, Header) {
	let source = FileSource::new(fs::File::open(path).unwrap()).unwrap();
	let mut image = Image::open(source, Limits::default()).unwrap();
	image.verify().unwrap();
	let mut header = [0; HEADER_BYTES];
	image
		.read_exact_at(PRIMARY_HEADER_OFFSET, &mut header)
		.unwrap();
	let header = Header::decode(&header, image.descriptor().logical_bytes).unwrap();
	(image, header)
}

// This fixture places each test fork in one contiguous extent and fits a single leaf.
fn fork_start(fork: &Fork, geometry: Geometry) -> u64 {
	assert_eq!(fork.extents().count(), 1);
	let extent = fork.extents().next().unwrap();
	geometry
		.block_range(extent.start, extent.blocks)
		.unwrap()
		.start
}

fn first_leaf(image: &mut Image<FileSource>, fork: &Fork, geometry: Geometry) -> Vec<u8> {
	let start = fork_start(fork, geometry);
	let mut prefix = [0; TREE_HEADER_PREFIX];
	image.read_exact_at(start, &mut prefix).unwrap();
	let node_bytes = usize::from(TreeHeader::node_size_hint(&prefix).unwrap());
	let mut node = vec![0; node_bytes];
	image.read_exact_at(start, &mut node).unwrap();
	let header = TreeHeader::decode(&Node::decode(&node).unwrap(), fork.logical_bytes).unwrap();
	assert_eq!(header.first_leaf, header.last_leaf);
	let leaf = header.first_leaf.unwrap();
	image
		.read_exact_at(start + u64::from(leaf.0) * node_bytes as u64, &mut node)
		.unwrap();
	node
}

fn assert_payloads(image: &mut Image<FileSource>, header: &Header) {
	let geometry = header.geometry;
	let catalog = first_leaf(image, &header.forks.catalog, geometry);
	let input = Node::decode(&catalog)
		.unwrap()
		.records()
		.map(|bytes| Record::decode(bytes, geometry).unwrap())
		.find(|record| record.key.name.units().eq(INPUT_FILE.encode_utf16()))
		.unwrap();
	let Value::File {
		metadata,
		data,
		resource,
	} = input.value
	else {
		panic!("the copied source is no longer a regular catalog file");
	};
	let mut stored = vec![0; INPUT.len()];
	assert_eq!(data.logical_bytes, stored.len() as u64);
	image
		.read_exact_at(fork_start(&data, geometry), &mut stored)
		.unwrap();
	assert_eq!(stored, INPUT);
	if cfg!(target_os = "macos") {
		let mut raw_resource = [0; RESOURCE_BYTES.len()];
		assert_eq!(resource.logical_bytes, raw_resource.len() as u64);
		image
			.read_exact_at(fork_start(&resource, geometry), &mut raw_resource)
			.unwrap();
		assert_eq!(&raw_resource, RESOURCE_BYTES);
	}
	let attrs = first_leaf(image, &header.forks.attributes, geometry);
	let attr = Node::decode(&attrs)
		.unwrap()
		.records()
		.map(|bytes| InlineRecord::decode(bytes).unwrap())
		.find(|record| {
			record.key.file_id == metadata.id
				&& record.key.name.units().eq(MARKER_NAME.encode_utf16())
		})
		.unwrap();
	assert_eq!(attr.value, MARKER_BYTES);
}

#[test]
fn applies_auto_capacity_and_includes_owned_attributes() {
	let temp = tempfile::tempdir().unwrap();
	let bytes_file = temp.path().join(INPUT_FILE);
	fs::write(&bytes_file, INPUT).unwrap();
	let descriptor = fs::File::open(&bytes_file).unwrap();
	fsetxattr(&descriptor, MARKER_NAME, MARKER_BYTES, XattrFlags::empty()).unwrap();
	if cfg!(target_os = "macos") {
		fsetxattr(
			&descriptor,
			RESOURCE_NAME,
			RESOURCE_BYTES,
			XattrFlags::empty(),
		)
		.unwrap();
	}
	let project = Project::parse(&format!(
		"{PROJECT_HEADER}free_space = {FREE_SPACE_BYTES}\n		 [metadata]\npolicy = 'report'\n		 [[contents]]\nsource = '{INPUT_FILE}'\n		 [[contents]]\ntype = 'symlink'\ndestination = '/Applications'\ntarget = '/Applications'\n"
	)).unwrap();
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let planned = prepared.seal().unwrap();
	let length = planned.logical_bytes();
	assert!(length > FREE_SPACE_BYTES);
	let report = planned.publish(None).unwrap();
	assert_eq!(length, report.report.image.logical_bytes);
	assert!(
		report
			.metadata_losses
			.iter()
			.all(|entry| entry.details.iter().all(|loss| {
				matches!(
					loss.reason,
					LossReason::TimestampPrecision { .. } | LossReason::OptionalStatUnavailable
				)
			}))
	);
	let (mut output, header) = open_output(&temp.path().join(IMAGE));
	let free = u64::from(header.free_blocks) * u64::from(header.geometry.block_size());
	assert!(free >= FREE_SPACE_BYTES);
	assert_ne!(header.forks.attributes.logical_bytes, 0);
	assert!(header.unmounted_cleanly());
	assert_payloads(&mut output, &header);
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
}

#[test]
fn refuses_a_source_mutation_before_publication() {
	let temp = tempfile::tempdir().unwrap();
	let input = temp.path().join(INPUT_FILE);
	fs::write(&input, INPUT).unwrap();
	let project = Project::parse(&format!(
		"{PROJECT_HEADER}size = '1MiB'\n[metadata]\npolicy = 'report'\n[[contents]]\nsource = '{INPUT_FILE}'\n"
	)).unwrap();
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let planned = prepared.seal().unwrap();
	fs::write(&input, b"replacement with a different length").unwrap();
	assert!(matches!(planned.publish(None), Err(Error::Execute(_))));
	assert!(!temp.path().join(IMAGE).exists());
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn refuses_known_unrepresentable_attributes_in_strict_mode() {
	let temp = tempfile::tempdir().unwrap();
	let bytes_file = temp.path().join(INPUT_FILE);
	fs::write(&bytes_file, INPUT).unwrap();
	let descriptor = fs::File::open(&bytes_file).unwrap();
	let marker = [0xAB; NATIVE_INLINE_MAX_VALUE_BYTES + 1];
	fsetxattr(descriptor, MARKER_NAME, &marker, XattrFlags::empty()).unwrap();
	let project = Project::parse(&format!(
		"{PROJECT_HEADER}[metadata]\npreserve_timestamps = false\n[[contents]]\nsource = '{INPUT_FILE}'\n"
	))
	.unwrap();
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	assert!(matches!(
		prepared.seal(),
		Err(Error::Policy { cause: dmgez_builder::metadata::policy::Error::Loss(loss), .. })
		if matches!(loss.field, dmgez_builder::metadata::policy::Field::ExtendedAttribute { .. })
	));
	assert!(!temp.path().join(IMAGE).exists());
}

#[test]
fn streams_deterministically_and_rejects_too_small_a_shrink_bound() {
	let temp = tempfile::tempdir().unwrap();
	let config = |maximum: &str| {
		Project::parse(&format!(
			"{PROJECT_HEADER}size = '{maximum}'\nshrink = true\nfree_space = {FREE_SPACE_BYTES}\n			 [build]\nreproducible = true\nsource_date_epoch = 946684800\n			 [[contents]]\ntype = 'directory'\ndestination = '/Équipement'\n"
		)).unwrap()
	};
	let project = config("1MiB");
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Stream),
		Default::default(),
	)
	.unwrap();
	let mut bytes = Vec::new();
	let report = prepared.seal().unwrap().write_to(&mut bytes).unwrap();
	assert!(report.report.image.logical_bytes < 1 << 20);
	assert!(report.metadata_losses.is_empty());
	let mut repeated = Vec::new();
	prepared.seal().unwrap().write_to(&mut repeated).unwrap();
	assert_eq!(bytes, repeated);
	assert!(!temp.path().join(IMAGE).exists());

	let project = config("64KiB");
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Stream),
		Default::default(),
	)
	.unwrap();
	assert!(matches!(prepared.seal(), Err(Error::Capacity(_))));
}

fn hard_link_project(preserve: bool) -> Project {
	Project::parse(&format!(
		"{PROJECT_HEADER}[metadata]\npreserve_timestamps = false\npreserve_hard_links = {preserve}\n\
		 [[contents]]\nsource = '{PRIMARY_SOURCE}'\n\
		 [[contents]]\nsource = '{LINK_SOURCE}'\ndestination = '/nested/{LINK_FILE}'\n\
		 [[contents]]\nsource = '{INDEPENDENT_FILE}'\n"
	))
	.unwrap()
}

fn create_hard_link_sources(root: &Path) {
	fs::create_dir(root.join(LINK_DIRECTORY)).unwrap();
	fs::create_dir(root.join(PRIMARY_DIRECTORY)).unwrap();
	fs::write(root.join(PRIMARY_SOURCE), INPUT).unwrap();
	fs::hard_link(root.join(PRIMARY_SOURCE), root.join(LINK_SOURCE)).unwrap();
	fs::write(root.join(INDEPENDENT_FILE), INPUT).unwrap();
}

#[test]
fn preserves_one_inode_and_reports_metadata_once_for_all_aliases() {
	let temp = tempfile::tempdir().unwrap();
	create_hard_link_sources(temp.path());
	let descriptor = fs::File::open(temp.path().join(PRIMARY_SOURCE)).unwrap();
	fsetxattr(&descriptor, MARKER_NAME, MARKER_BYTES, XattrFlags::empty()).unwrap();
	if cfg!(target_os = "macos") {
		fsetxattr(
			&descriptor,
			RESOURCE_NAME,
			RESOURCE_BYTES,
			XattrFlags::empty(),
		)
		.unwrap();
	}
	let project = hard_link_project(true);
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let report = prepared.seal().unwrap().publish(None).unwrap();
	assert!(
		report
			.metadata_losses
			.iter()
			.flat_map(|loss| &loss.details)
			.all(|loss| !matches!(loss.field, Field::HardLink { .. }))
	);
	let shared_loss = report
		.metadata_losses
		.iter()
		.find(|loss| !loss.aliases.is_empty())
		.unwrap();
	assert_eq!(shared_loss.path.as_str(), format!("/nested/{LINK_FILE}"));
	assert_eq!(shared_loss.aliases[0].as_str(), format!("/{INPUT_FILE}"));
	assert_eq!(shared_loss.aliases.len(), 1);

	let (mut output, header) = open_output(&temp.path().join(IMAGE));
	let geometry = header.geometry;
	let catalog = first_leaf(&mut output, &header.forks.catalog, geometry);
	let node = Node::decode(&catalog).unwrap();
	let record = |name: &str| {
		node.records()
			.map(|bytes| Record::decode(bytes, geometry).unwrap())
			.find(|record| record.key.name.units().eq(name.encode_utf16()))
			.unwrap()
	};
	let primary = Stub::decode(&record(INPUT_FILE)).unwrap().unwrap();
	let alias = Stub::decode(&record(LINK_FILE)).unwrap().unwrap();
	assert_eq!(primary.reference, alias.reference);
	assert_eq!(primary.neighbors, None);
	assert_eq!(alias.neighbors, None);
	assert!(Stub::decode(&record(INDEPENDENT_FILE)).unwrap().is_none());
	let id = |value| match value {
		Value::File { metadata, .. } | Value::Folder { metadata, .. } => metadata.id,
		_ => panic!("expected a visible catalog record"),
	};
	assert_ne!(id(record(INPUT_FILE).value), id(record(LINK_FILE).value));
	let private = node
		.records()
		.map(|bytes| Record::decode(bytes, geometry).unwrap())
		.find(|record| is_private_directory_key(record.key))
		.unwrap();
	let inode = node
		.records()
		.filter_map(|bytes| Inode::decode(bytes, id(private.value), geometry).unwrap())
		.find(|inode| inode.reference == primary.reference)
		.unwrap();
	assert_eq!(inode.link_count.get(), 2);
	assert_ne!(inode.metadata.id, id(record(INPUT_FILE).value));
	assert_ne!(inode.metadata.id, id(record(LINK_FILE).value));
	assert_eq!(inode.data.logical_bytes, INPUT.len() as u64);
	let mut data = [0; INPUT.len()];
	output
		.read_exact_at(fork_start(&inode.data, geometry), &mut data)
		.unwrap();
	assert_eq!(data, INPUT);
	if cfg!(target_os = "macos") {
		let mut data = [0; RESOURCE_BYTES.len()];
		assert_eq!(inode.resource.logical_bytes, RESOURCE_BYTES.len() as u64);
		output
			.read_exact_at(fork_start(&inode.resource, geometry), &mut data)
			.unwrap();
		assert_eq!(data, RESOURCE_BYTES);
	}
	let attrs = first_leaf(&mut output, &header.forks.attributes, geometry);
	let attributes = Node::decode(&attrs).unwrap();
	let mut marker = attributes
		.records()
		.map(|bytes| InlineRecord::decode(bytes).unwrap())
		.filter(|record| record.key.name.units().eq(MARKER_NAME.encode_utf16()));
	let shared = marker.next().unwrap();
	assert_eq!(shared.key.file_id, inode.metadata.id);
	assert_eq!(shared.value, MARKER_BYTES);
	assert!(marker.next().is_none());

	let mut reversed = hard_link_project(true);
	reversed.contents.reverse();
	let prepared = hfsplus::prepare(
		&reversed,
		temp.path(),
		options(OutputIntent::Stream),
		Default::default(),
	)
	.unwrap();
	let mut rebuilt = Vec::new();
	prepared.seal().unwrap().write_to(&mut rebuilt).unwrap();
	assert_eq!(rebuilt, fs::read(temp.path().join(IMAGE)).unwrap());
}

#[test]
fn reports_explicit_hard_link_omission_and_writes_independent_files() {
	let temp = tempfile::tempdir().unwrap();
	create_hard_link_sources(temp.path());
	let project = hard_link_project(false);
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let report = prepared.seal().unwrap().publish(None).unwrap();
	let omissions = report
		.metadata_losses
		.iter()
		.flat_map(|loss| &loss.details)
		.filter(|loss| {
			matches!(loss.field, Field::HardLink { .. })
				&& loss.reason == LossReason::ConfiguredOmission
		})
		.count();
	assert_eq!(omissions, 2);
	let (mut output, header) = open_output(&temp.path().join(IMAGE));
	let catalog = first_leaf(&mut output, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	assert!(node.records().all(|bytes| {
		let record = Record::decode(bytes, header.geometry).unwrap();
		Stub::decode(&record).unwrap().is_none()
	}));
}

#[test]
fn refuses_replacement_of_a_nonrepresentative_alias_before_publication() {
	let temp = tempfile::tempdir().unwrap();
	create_hard_link_sources(temp.path());
	let project = hard_link_project(true);
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let image = prepared.seal().unwrap();
	// The representative sorts as /nested/linked.txt. Replace the other alias's
	// containing directory by renaming the original tree rather than unlinking
	// either hard link, which would also change the representative inode's ctime.
	let parent = temp.path().join(PRIMARY_DIRECTORY);
	fs::rename(&parent, temp.path().join("moved-selection")).unwrap();
	fs::create_dir(&parent).unwrap();
	let replacement = parent.join(INPUT_FILE);
	fs::write(&replacement, INPUT).unwrap();
	let replacement = replacement.canonicalize().unwrap();
	assert!(matches!(
		image.publish(None),
		Err(Error::Execute(ExecuteError::BeforeOutput(ImageError::Source(
			manifest::Error::SourceChanged { path }
		)))) if path == replacement
	));
	assert!(!temp.path().join(IMAGE).exists());
}
