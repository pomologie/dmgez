use std::{
	io::Write,
	process::{Command, Stdio},
};

use dmgez_builder::progress::BuildProgress;
use hfsplus_format::{catalog::Metadata, volume::CatalogId};

use super::{
	Error, ExecuteError, FileSource, Fork, Header, IMAGE, Image, Node, OutputIntent, Path, Project,
	Record, Value, first_leaf, fork_start, fs, hfsplus, open_output, options,
};

#[path = "retina_background.rs"]
mod retina;

const PROJECT: &str = include_str!("../fixtures/finder-background.toml");
const ROOT_SOURCE: &str = "background-root.png";
const OVERRIDE_SOURCE: &str = "background-override.png";
const ROOT_BYTES: &[u8] = include_bytes!("../fixtures/background-root.png");
const OVERRIDE_BYTES: &[u8] = include_bytes!("../fixtures/background-override.png");
const BACKGROUND_FOLDER: &str = ".background";
const STORE: &str = ".DS_Store";
const ROOT_PARENT_ID: CatalogId = CatalogId(1);
const ROOT_ID: CatalogId = CatalogId(2);
const INHERITED_FOLDER: &str = "Inherited";
const OVERRIDE_FOLDER: &str = "Override";
const SOLID_FOLDER: &str = "Solid";
const FINDER_FLAGS: std::ops::Range<usize> = 8..10;
const FINDER_INVISIBLE: u16 = 0x4000;
const CREATED_DATE: usize = 0;
const EXPECTED_HFS_DATE: u32 = 3_029_529_600;
const ORACLE: &str = include_str!("background_oracle.py");

fn write_sources(root: &Path) {
	fs::write(root.join(ROOT_SOURCE), ROOT_BYTES).unwrap();
	fs::write(root.join(OVERRIDE_SOURCE), OVERRIDE_BYTES).unwrap();
}

fn publish_fixture(root: &Path) -> (Image<FileSource>, Header) {
	write_sources(root);
	publish_project(root, &Project::parse(PROJECT).unwrap());
	open_output(&root.join(IMAGE))
}

fn publish_project(root: &Path, project: &Project) {
	hfsplus::prepare(
		project,
		root,
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap()
	.seal()
	.unwrap()
	.publish(None)
	.unwrap();
}

fn read_fork(image: &mut Image<FileSource>, header: &Header, fork: &Fork) -> Vec<u8> {
	let mut bytes = vec![0; usize::try_from(fork.logical_bytes).unwrap()];
	image
		.read_exact_at(fork_start(fork, header.geometry), &mut bytes)
		.unwrap();
	bytes
}

fn catalog_records<'a>(node: &'a Node<'a>, header: &Header) -> impl Iterator<Item = Record<'a>> {
	let geometry = header.geometry;
	node.records()
		.map(move |bytes| Record::decode(bytes, geometry).unwrap())
}

fn folder(node: &Node<'_>, header: &Header, parent: CatalogId, name: &str) -> Metadata {
	catalog_records(node, header)
		.find_map(|record| match record.value {
			Value::Folder { metadata, .. }
				if record.key.parent_or_thread == parent
					&& record.key.name.units().eq(name.encode_utf16()) =>
			{
				Some(metadata)
			}
			_ => None,
		})
		.unwrap_or_else(|| panic!("missing folder {name} below CNID {}", parent.0))
}

fn assert_invisible(metadata: Metadata) {
	let flags = u16::from_be_bytes(metadata.finder_info[FINDER_FLAGS].try_into().unwrap());
	assert_eq!(flags & FINDER_INVISIBLE, FINDER_INVISIBLE);
}

#[derive(Debug)]
struct Asset {
	name: String,
	metadata: Metadata,
	parent: CatalogId,
}

fn assert_assets(image: &mut Image<FileSource>, node: &Node<'_>, header: &Header) -> [Asset; 2] {
	let background = folder(node, header, ROOT_ID, BACKGROUND_FOLDER);
	assert_invisible(background);
	let mut root = None;
	let mut overridden = None;
	for record in catalog_records(node, header) {
		let Value::File { metadata, data, .. } = record.value else {
			continue;
		};
		if record.key.parent_or_thread != background.id {
			continue;
		}
		assert_invisible(metadata);
		assert_eq!(metadata.dates[CREATED_DATE], EXPECTED_HFS_DATE);
		let path = String::from_utf16(&record.key.name.units().collect::<Vec<_>>()).unwrap();
		assert!(path.ends_with(".png"));
		let bytes = read_fork(image, header, &data);
		let slot = match bytes.as_slice() {
			ROOT_BYTES => &mut root,
			OVERRIDE_BYTES => &mut overridden,
			_ => panic!("unexpected background asset {path}"),
		};
		assert!(slot.is_none(), "duplicated background asset {path}");
		*slot = Some(Asset {
			name: path,
			metadata,
			parent: record.key.parent_or_thread,
		});
	}
	let assets = [root.unwrap(), overridden.unwrap()];
	assert_ne!(assets[0].metadata.id, assets[1].metadata.id);
	assets
}

fn store_fork(node: &Node<'_>, header: &Header, parent: CatalogId) -> Fork {
	catalog_records(node, header)
		.find_map(|record| match record.value {
			Value::File { data, .. }
				if record.key.parent_or_thread == parent
					&& record.key.name.units().eq(STORE.encode_utf16()) =>
			{
				Some(data)
			}
			_ => None,
		})
		.unwrap_or_else(|| panic!("missing Finder store in CNID {}", parent.0))
}

fn store_folders(node: &Node<'_>, header: &Header) -> [CatalogId; 4] {
	let inherited = folder(node, header, ROOT_ID, INHERITED_FOLDER).id;
	let overridden = folder(node, header, inherited, OVERRIDE_FOLDER).id;
	let solid = folder(node, header, ROOT_ID, SOLID_FOLDER).id;
	[ROOT_ID, inherited, overridden, solid]
}

#[test]
fn reserves_hidden_backgrounds_and_preserves_exact_payloads() {
	let temp = tempfile::tempdir().unwrap();
	let (mut image, header) = publish_fixture(temp.path());
	assert_eq!(header.dates[CREATED_DATE], EXPECTED_HFS_DATE);
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let _ = assert_assets(&mut image, &node, &header);
	for parent in store_folders(&node, &header) {
		assert_ne!(store_fork(&node, &header, parent).logical_bytes, 0);
	}
	assert!(header.unmounted_cleanly());
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_oracle_matches_background_aliases_to_the_hfs_catalog() {
	let temp = tempfile::tempdir().unwrap();
	let (mut image, header) = publish_fixture(temp.path());
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let assets = assert_assets(&mut image, &node, &header);
	assert_fixture_aliases(&mut image, &node, &header, &assets);
}

fn assert_fixture_aliases(
	image: &mut Image<FileSource>,
	node: &Node<'_>,
	header: &Header,
	[root_asset, override_asset]: &[Asset; 2],
) {
	let root = catalog_records(node, header)
		.find(|record| {
			record.key.parent_or_thread == ROOT_PARENT_ID
				&& matches!(record.value, Value::Folder { metadata, .. } if metadata.id == ROOT_ID)
		})
		.unwrap();
	let volume_name = String::from_utf16(&root.key.name.units().collect::<Vec<_>>()).unwrap();
	let expected = [
		("root", Some(root_asset)),
		("inherited", Some(root_asset)),
		("override", Some(override_asset)),
		("color", None),
	];
	for (parent, (kind, asset)) in store_folders(node, header).into_iter().zip(expected) {
		let data = store_fork(node, header, parent);
		let bytes = read_fork(image, header, &data);
		let mut command = Command::new("python3");
		command.args(["-B", "-c", ORACLE, kind]);
		if let Some(asset) = asset {
			command.args([
				volume_name.as_str(),
				&header.dates[CREATED_DATE].to_string(),
				&asset.name,
				&asset.metadata.id.0.to_string(),
				&asset.parent.0.to_string(),
				&asset.metadata.dates[CREATED_DATE].to_string(),
				&format!("/{BACKGROUND_FOLDER}/{}", asset.name),
			]);
		}
		let mut oracle = command.stdin(Stdio::piped()).spawn().unwrap();
		oracle.stdin.take().unwrap().write_all(&bytes).unwrap();
		assert!(oracle.wait().unwrap().success(), "oracle failed for {kind}");
	}
}

#[test]
fn refuses_malformed_background_data_before_publication() {
	let temp = tempfile::tempdir().unwrap();
	write_sources(temp.path());
	let truncated_png = &ROOT_BYTES[..ROOT_BYTES.len() / 2];
	fs::write(temp.path().join(ROOT_SOURCE), truncated_png).unwrap();
	let project = Project::parse(PROJECT).unwrap();
	assert!(matches!(
		hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		),
		Err(Error::Prepare(
			dmgez_builder::planning::Error::Presentation(_)
		))
	));
	assert!(!temp.path().join(IMAGE).exists());
}

#[test]
fn changing_a_background_during_publication_preserves_existing_output() {
	const PREVIOUS_IMAGE: &[u8] = b"previous image fixture";
	let temp = tempfile::tempdir().unwrap();
	let source = temp.path().join(ROOT_SOURCE);
	let output = temp.path().join(IMAGE);
	write_sources(temp.path());
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let project = Project::parse(&PROJECT.replace("[build]", "[build]\noverwrite = true")).unwrap();
	let prepared = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let result = prepared
		.seal()
		.unwrap()
		.publish_with_progress(None, |progress| {
			if matches!(progress, BuildProgress::Publishing) {
				fs::write(&source, b"changed during publication").unwrap();
			}
		});
	assert!(matches!(
		result,
		Err(Error::Execute(ExecuteError::Replace(_)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
}
