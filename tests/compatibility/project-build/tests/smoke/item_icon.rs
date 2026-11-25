use dmgez_builder::{preflight::Availability, presentation};
use hfsplus_format::{
	catalog::Value,
	volume::{Fork, Geometry},
};

use super::{
	BuildProgress, CUSTOM_ICON, Error, ExecuteError, FileSource, ICON_ASSET, ICON_BYTES,
	ICON_SOURCE, IMAGE, Image, MARKER_BYTES, MARKER_NAME, Node, OutputIntent, PREVIOUS_IMAGE,
	PROJECT_HEADER, Path, Project, REPRODUCIBLE, ROOT_FINDER_FLAGS, ROOT_ID, Record, XattrFlags,
	first_leaf, fork_start, fs, fsetxattr, hfsplus, manifest, open_output, options, planning,
};

const ICON_CHILD: &str = "Icon\r";
const FOLDER: &str = "Manual";
const FILE: &str = "Readme.txt";
const UNRELATED: &str = "Unrelated.txt";
const NOTE: &str = "Notes.txt";
const FILE_BYTES: &[u8] = b"keep the data fork unchanged";
const UNRELATED_BYTES: &[u8] = b"unmodified sibling";
const NOTE_BYTES: &[u8] = b"folder content";
const INVISIBLE: u16 = 0x4000;
const HAS_BEEN_INITED: u16 = 0x0100;
const FINDER_INFO_BYTES: usize = 32;
const FINDER_TYPE_CREATOR_BYTES: usize = 8;
const FINDER_INFO_NAME: &str = "com.apple.FinderInfo";
const FILE_ICON: &str = "[[contents]]\nsource = 'Readme.txt'\ncustom_icon = 'source.icns'\n";
const FOLDER_ICON: &str =
	"[[contents]]\ntype = 'directory'\ndestination = '/Manual'\ncustom_icon = 'source.icns'\n";

// The resource fork has a 16-byte header followed by data and a resource map.
// Map type references are relative to the type list. The low 24 bits of a
// resource reference's attribute-and-offset word address its data record.
const MAP_OFFSET_FIELD: usize = 4;
const DATA_LENGTH_FIELD: usize = 8;
const MAP_LENGTH_FIELD: usize = 12;
const TYPE_LIST_OFFSET_FIELD: usize = 24;
const TYPE_RECORD_START: usize = 2;
const TYPE_AND_CREATOR_LENGTH: usize = 4;
const REFERENCE_LIST_OFFSET_FIELD: usize = 6;
const REFERENCE_NAME_FIELD: usize = 2;
const REFERENCE_ATTRIBUTES_AND_OFFSET_FIELD: usize = 4;
const DATA_OFFSET_MASK: u32 = 0x00ff_ffff;
const RESOURCE_LENGTH_BYTES: usize = 4;
const CUSTOM_ICON_ID: i16 = -16455;
const ICNS_TYPE: &[u8; 4] = b"icns";

fn project(contents: &str, metadata: &str) -> Project {
	Project::parse(&format!(
		"{PROJECT_HEADER}{REPRODUCIBLE}overwrite = true\n[metadata]\npreserve_timestamps = false\n{metadata}{contents}"
	))
	.unwrap()
}

fn source_files(root: &Path) {
	fs::write(root.join(ICON_SOURCE), ICON_BYTES).unwrap();
	fs::write(root.join(FILE), FILE_BYTES).unwrap();
}

fn be_u16(bytes: &[u8], offset: usize) -> u16 {
	u16::from_be_bytes(bytes[offset..][..2].try_into().unwrap())
}

fn be_u32(bytes: &[u8], offset: usize) -> u32 {
	u32::from_be_bytes(bytes[offset..][..4].try_into().unwrap())
}

fn assert_icon_resource(fork: &[u8]) {
	let data_offset = be_u32(fork, 0) as usize;
	let map_offset = be_u32(fork, MAP_OFFSET_FIELD) as usize;
	let data_length = be_u32(fork, DATA_LENGTH_FIELD) as usize;
	let map_length = be_u32(fork, MAP_LENGTH_FIELD) as usize;
	let data = &fork[data_offset..data_offset + data_length];
	let map = &fork[map_offset..map_offset + map_length];
	let types = &map[usize::from(be_u16(map, TYPE_LIST_OFFSET_FIELD))..];
	assert_eq!(be_u16(types, 0), 0, "expected exactly one resource type");
	let record = &types[TYPE_RECORD_START..];
	assert_eq!(&record[..TYPE_AND_CREATOR_LENGTH], ICNS_TYPE);
	assert_eq!(
		be_u16(record, TYPE_AND_CREATOR_LENGTH),
		0,
		"expected exactly one icon resource"
	);
	let reference = &types[usize::from(be_u16(record, REFERENCE_LIST_OFFSET_FIELD))..];
	assert_eq!(
		i16::from_be_bytes(reference[..2].try_into().unwrap()),
		CUSTOM_ICON_ID
	);
	assert_eq!(be_u16(reference, REFERENCE_NAME_FIELD), u16::MAX);
	let offset =
		(be_u32(reference, REFERENCE_ATTRIBUTES_AND_OFFSET_FIELD) & DATA_OFFSET_MASK) as usize;
	let length = be_u32(data, offset) as usize;
	let start = offset + RESOURCE_LENGTH_BYTES;
	assert_eq!(&data[start..start + length], ICON_BYTES);
}

fn read_fork(image: &mut Image<FileSource>, fork: &Fork, geometry: Geometry) -> Vec<u8> {
	let mut bytes = vec![0; usize::try_from(fork.logical_bytes).unwrap()];
	if !bytes.is_empty() {
		image
			.read_exact_at(fork_start(fork, geometry), &mut bytes)
			.unwrap();
	}
	bytes
}

#[test]
fn writes_file_and_folder_icons_without_changing_data_or_unrelated_metadata() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	fs::write(root.join(UNRELATED), UNRELATED_BYTES).unwrap();
	fs::write(root.join(NOTE), NOTE_BYTES).unwrap();
	let input = fs::File::open(root.join(FILE)).unwrap();
	fsetxattr(&input, MARKER_NAME, MARKER_BYTES, XattrFlags::empty()).unwrap();
	let mut source_info = [0; FINDER_INFO_BYTES];
	let source_flags = if cfg!(target_os = "macos") {
		HAS_BEEN_INITED
	} else {
		0
	};
	if cfg!(target_os = "macos") {
		source_info[..FINDER_TYPE_CREATOR_BYTES].copy_from_slice(b"TEXTttxt");
		source_info[ROOT_FINDER_FLAGS].copy_from_slice(&source_flags.to_be_bytes());
		fsetxattr(&input, FINDER_INFO_NAME, &source_info, XattrFlags::empty()).unwrap();
	}
	let config = project(
		&format!(
			"{FILE_ICON}{FOLDER_ICON}[[contents]]\nsource = '{UNRELATED}'\n\
			[[contents]]\nsource = '{NOTE}'\ndestination = '/{FOLDER}/{NOTE}'\n"
		),
		"",
	);
	let prepared = hfsplus::prepare(
		&config,
		root,
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	assert!(!root.join(IMAGE).exists());
	let sealed = prepared.seal().unwrap();
	assert!(!root.join(IMAGE).exists());
	sealed.publish(None).unwrap();
	assert_eq!(fs::read(root.join(FILE)).unwrap(), FILE_BYTES);
	assert_eq!(fs::read(root.join(ICON_SOURCE)).unwrap(), ICON_BYTES);

	let (mut image, header) = open_output(&root.join(IMAGE));
	let geometry = header.geometry;
	let leaf = first_leaf(&mut image, &header.forks.catalog, geometry);
	let node = Node::decode(&leaf).unwrap();
	let records = || {
		node.records()
			.map(|bytes| Record::decode(bytes, geometry).unwrap())
	};
	let read = |parent, name: &str| {
		records()
			.find(|record| {
				record.key.parent_or_thread == parent
					&& record.key.name.units().eq(name.encode_utf16())
			})
			.unwrap()
			.value
	};
	let Value::File {
		metadata,
		data,
		resource,
	} = read(ROOT_ID, FILE)
	else {
		panic!("the custom file icon must stay on a regular file");
	};
	let file_identity = metadata.id;
	source_info[ROOT_FINDER_FLAGS].copy_from_slice(&(CUSTOM_ICON | source_flags).to_be_bytes());
	assert_eq!(metadata.finder_info, source_info);
	assert_eq!(read_fork(&mut image, &data, geometry), FILE_BYTES);
	assert_icon_resource(&read_fork(&mut image, &resource, geometry));

	let Value::Folder { metadata, .. } = read(ROOT_ID, FOLDER) else {
		panic!("the custom folder icon must stay on a folder");
	};
	let folder_identity = metadata.id;
	assert_eq!(
		u16::from_be_bytes(metadata.finder_info[ROOT_FINDER_FLAGS].try_into().unwrap()),
		CUSTOM_ICON
	);
	let Value::File {
		metadata,
		data,
		resource,
	} = read(folder_identity, ICON_CHILD)
	else {
		panic!("the folder icon payload must occupy an invisible catalog child");
	};
	assert_eq!(
		metadata.finder_info[..FINDER_TYPE_CREATOR_BYTES],
		[0; FINDER_TYPE_CREATOR_BYTES]
	);
	assert_eq!(
		u16::from_be_bytes(metadata.finder_info[ROOT_FINDER_FLAGS].try_into().unwrap()),
		INVISIBLE
	);
	assert_eq!(data.logical_bytes, 0);
	assert_icon_resource(&read_fork(&mut image, &resource, geometry));
	let Value::File { data, resource, .. } = read(folder_identity, NOTE) else {
		panic!("the folder's ordinary child must coexist with its icon");
	};
	assert_eq!(read_fork(&mut image, &data, geometry), NOTE_BYTES);
	assert_eq!(resource.logical_bytes, 0);

	let Value::File {
		metadata,
		data,
		resource,
	} = read(ROOT_ID, UNRELATED)
	else {
		panic!("the unrelated file must stay in the catalog");
	};
	assert_eq!(read_fork(&mut image, &data, geometry), UNRELATED_BYTES);
	assert_eq!(resource.logical_bytes, 0);
	assert_eq!(metadata.finder_info, [0; FINDER_INFO_BYTES]);
	assert!(!records().any(|record| record.key.name.units().eq(ICON_ASSET.encode_utf16())));
	assert!(records().any(|record| {
		matches!(record.value, Value::Folder { metadata, .. }
			if metadata.id == ROOT_ID && metadata.finder_info == [0; FINDER_INFO_BYTES])
	}));

	let attrs = first_leaf(&mut image, &header.forks.attributes, geometry);
	let attrs = Node::decode(&attrs).unwrap();
	let marker = attrs
		.records()
		.map(|record| hfsplus_format::attributes::InlineRecord::decode(record).unwrap())
		.find(|attr| {
			attr.key.file_id == file_identity
				&& attr.key.name.units().eq(MARKER_NAME.encode_utf16())
		})
		.unwrap();
	assert_eq!(marker.value, MARKER_BYTES);
}

#[test]
fn protects_the_item_icon_source_from_output_alias_and_late_mutation() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	let input = root.join(ICON_SOURCE);
	let output = root.join(IMAGE);
	let config = project(FILE_ICON, "");
	let prepare = || {
		let mut profile = options(OutputIntent::Path);
		profile.capabilities.hfsplus_unicode = Availability::Unavailable;
		hfsplus::prepare(&config, root, profile, Default::default())
	};
	fs::hard_link(&input, &output).unwrap();
	assert!(matches!(
		prepare(),
		Err(Error::Prepare(planning::Error::Presentation(
			presentation::Error::ItemIconSource(manifest::Error::OutputSourceAlias { .. })
		)))
	));
	assert_eq!(fs::read(&output).unwrap(), ICON_BYTES);
	fs::remove_file(&output).unwrap();
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let prepared = prepare().unwrap();
	let sealed = prepared.seal().unwrap();
	let result = sealed.publish_with_progress(None, |progress| {
		if matches!(progress, BuildProgress::Publishing) {
			fs::write(&input, b"mutated staged item icon source").unwrap();
		}
	});
	assert!(matches!(
		result,
		Err(Error::Execute(ExecuteError::Replace(_)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	assert_eq!(fs::read_dir(root).unwrap().count(), 3);
}

#[test]
fn rejects_malformed_icns_without_replacing_an_image() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	fs::write(root.join(ICON_SOURCE), &ICON_BYTES[..ICON_BYTES.len() / 2]).unwrap();
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let config = project(FILE_ICON, "");
	assert!(matches!(
		hfsplus::prepare(&config, root, options(OutputIntent::Path), Default::default()),
		Err(Error::Prepare(planning::Error::Presentation(
			presentation::Error::ItemIcon { path, .. }
		))) if path.as_str() == format!("/{FILE}")
	));
	assert_eq!(fs::read(output).unwrap(), PREVIOUS_IMAGE);
}

#[test]
fn bounds_item_icon_workspace_and_retained_resource_forks() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	for (limits, metadata_is_limited) in [
		("[performance]\nmax_metadata_bytes = '16KiB'\n", true),
		("[performance]\nmemory_limit = '32MiB'\n", false),
	] {
		let config = project(FILE_ICON, limits);
		let failure = hfsplus::prepare(
			&config,
			root,
			options(OutputIntent::Path),
			Default::default(),
		);
		let rejected_for_metadata = match failure {
			Err(Error::Prepare(planning::Error::Presentation(
				presentation::Error::MetadataBudget,
			))) => true,
			Err(Error::Prepare(planning::Error::Presentation(
				presentation::Error::ItemIconMemory,
			))) => false,
			_ => panic!("expected a budget error for {limits}"),
		};
		assert_eq!(rejected_for_metadata, metadata_is_limited);
		assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	}
}

#[test]
fn refuses_icons_for_symlinks_apps_and_non_unicode_folders() {
	const EXPLICIT_LINK: &str = "[[contents]]\ntype = 'symlink'\ndestination = '/Link'\ntarget = 'Readme.txt'\ncustom_icon = 'source.icns'\n";
	const COPIED_LINK: &str = "[[contents]]\nsource = 'HostLink'\ncustom_icon = 'source.icns'\n";
	const APP_DESTINATION: &str = "[[contents]]\nsource = 'Readme.txt'\ndestination = '/Editor.APP/Readme.txt'\ncustom_icon = 'source.icns'\n";
	const APP_SOURCE: &str = "[[contents]]\nsource = 'Editor.App/Contents/document.txt'\ndestination = '/Moved.txt'\ncustom_icon = 'source.icns'\n";
	const RENAMED_BUNDLE: &str =
		"[[contents]]\nsource = 'Renamed'\ndestination = '/Copied'\ncustom_icon = 'source.icns'\n";
	const APP_DOCUMENT: &str = "Editor.App/Contents/document.txt";
	const CODE_RESOURCES: &str = "Renamed/Contents/_CodeSignature/CodeResources";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	fs::write(root.join(FILE), FILE_BYTES).unwrap();
	std::os::unix::fs::symlink(FILE, root.join("HostLink")).unwrap();
	for name in [APP_DOCUMENT, CODE_RESOURCES] {
		let source = root.join(name);
		fs::create_dir_all(source.parent().unwrap()).unwrap();
		fs::write(source, FILE_BYTES).unwrap();
	}
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let ascii_only = || {
		let mut profile = options(OutputIntent::Path);
		profile.capabilities.hfsplus_unicode = Availability::Unavailable;
		profile
	};
	for (contents, destination, profile, signed_bundle) in [
		(EXPLICIT_LINK, "/Link", options(OutputIntent::Path), false),
		(COPIED_LINK, "/HostLink", options(OutputIntent::Path), false),
		(
			APP_DESTINATION,
			"/Editor.APP/Readme.txt",
			options(OutputIntent::Path),
			true,
		),
		(APP_SOURCE, "/Moved.txt", options(OutputIntent::Path), true),
		(RENAMED_BUNDLE, "/Copied", options(OutputIntent::Path), true),
		(FOLDER_ICON, "/Manual", ascii_only(), false),
	] {
		let config = project(contents, "");
		let result = hfsplus::prepare(&config, root, profile, Default::default());
		let (path, protected) = match result {
			Err(Error::Prepare(planning::Error::Presentation(
				presentation::Error::ItemIconTarget { path },
			))) => (path, false),
			Err(Error::Prepare(planning::Error::Presentation(
				presentation::Error::ItemIconSignedBundle { path },
			))) => (path, true),
			_ => panic!("expected item icon rejection for {destination} before reading the icon"),
		};
		assert_eq!(path.as_str(), destination);
		assert_eq!(protected, signed_bundle, "{destination}");
		assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	}
	assert_eq!(fs::read(root.join(APP_DOCUMENT)).unwrap(), FILE_BYTES);
	assert_eq!(fs::read(root.join(CODE_RESOURCES)).unwrap(), FILE_BYTES);
}

#[test]
fn refuses_to_replace_a_copied_native_icon_child() {
	const COPIED_FOLDER: &str = "[[contents]]\nsource = 'Manual'\ncustom_icon = 'source.icns'\n";
	const COLLIDING_CHILD: &str = "iCoN\r";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	let folder = root.join(FOLDER);
	fs::create_dir(&folder).unwrap();
	let existing = folder.join(COLLIDING_CHILD);
	fs::write(&existing, FILE_BYTES).unwrap();
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let config = project(COPIED_FOLDER, "policy = 'report'\n");
	let prepared = hfsplus::prepare(
		&config,
		root,
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	let Err(Error::Filesystem(collision)) = prepared.seal() else {
		panic!("the HFS+ planner must reject the case-equivalent icon child");
	};
	let detail = collision.to_string();
	let copied_path = format!("{FOLDER}/{COLLIDING_CHILD}");
	let generated_path = format!("{FOLDER}/{ICON_CHILD}");
	assert!(detail.contains("equivalent HFS+ catalog paths"), "{detail}");
	assert!(detail.contains(&format!("{copied_path:?}")));
	assert!(detail.contains(&format!("{generated_path:?}")));
	assert_eq!(fs::read(&existing).unwrap(), FILE_BYTES);
	assert_eq!(fs::read(output).unwrap(), PREVIOUS_IMAGE);
}

#[test]
fn refuses_shared_hard_link_items_before_replacing_an_image() {
	const HARD_LINK: &str = "Hardlink.txt";
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	fs::hard_link(root.join(FILE), root.join(HARD_LINK)).unwrap();
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let config = project(
		&format!("{FILE_ICON}[[contents]]\nsource = '{HARD_LINK}'\n"),
		"",
	);
	let prepared = hfsplus::prepare(
		&config,
		root,
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	assert!(
		matches!(prepared.seal(), Err(Error::CustomIconHardLink { path }) if path == format!("/{FILE}"))
	);
	assert_eq!(fs::read(output).unwrap(), PREVIOUS_IMAGE);
}

#[cfg(target_os = "macos")]
#[test]
fn refuses_to_replace_an_existing_resource_fork_even_when_omitted_from_the_output_policy() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	source_files(root);
	let input = fs::File::open(root.join(FILE)).unwrap();
	fsetxattr(
		&input,
		super::RESOURCE_NAME,
		super::RESOURCE_BYTES,
		XattrFlags::empty(),
	)
	.unwrap();
	let output = root.join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let config = project(FILE_ICON, "preserve_resource_forks = false\n");
	let prepared = hfsplus::prepare(
		&config,
		root,
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();
	assert!(
		matches!(prepared.seal(), Err(Error::CustomIconResourceFork { path }) if path == format!("/{FILE}"))
	);
	assert_eq!(fs::read(output).unwrap(), PREVIOUS_IMAGE);
}
