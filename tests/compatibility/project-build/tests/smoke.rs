#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::{fs, num::NonZeroUsize, path::Path};

use dmgez_builder::{
	metadata::policy::LossReason,
	preflight::{OutputIntent, PreflightOptions, ReleaseIntent},
};
use dmgez_config::Project;
use dmgez_hfs_adapter_probe::hfsplus::{self, Error};
use dmgez_udif::reader::{FileSource, Image, Limits};
use hfsplus_format::{
	attributes::{InlineRecord, NATIVE_INLINE_MAX_VALUE_BYTES},
	btree::{Node, TreeHeader},
	catalog::{Record, Value},
	volume::{Fork, Geometry, HEADER_BYTES, Header, PRIMARY_HEADER_OFFSET},
};
use rustix::fs::{XattrFlags, fsetxattr};

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
