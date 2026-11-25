use dmgez_builder::{planning, presentation, progress::BuildProgress};
use hfsplus_format::volume::CatalogId;

use super::*;

const ICON_SOURCE: &str = "source.icns";
const ICON_ASSET: &str = ".VolumeIcon.icns";
const ICON_BYTES: &[u8] = include_bytes!("apple-iconutil.icns");
const ROOT_ID: CatalogId = CatalogId(2);
const ROOT_FINDER_FLAGS: std::ops::Range<usize> = 8..10;
const CUSTOM_ICON: u16 = 0x0400;

fn project(settings: &str) -> Project {
	Project::parse(&format!(
		"{PROJECT_HEADER}[volume_icon]\nsource = '{ICON_SOURCE}'\n{settings}"
	))
	.unwrap()
}

#[test]
fn prebuilt_icon_agrees_with_the_catalog_and_preserves_all_bytes() {
	let temp = tempfile::tempdir().unwrap();
	fs::write(temp.path().join(ICON_SOURCE), ICON_BYTES).unwrap();
	let project = project("");
	let build = hfsplus::prepare(
		&project,
		temp.path(),
		options(OutputIntent::Path),
		Default::default(),
	)
	.unwrap();

	let prepared = build.prepared();
	assert!(!temp.path().join(IMAGE).exists());
	assert_eq!(prepared.manifest().entries().len(), 0);
	assert_eq!(prepared.finder_assets().len(), 2);
	let mut root_info = [0x20; 32];
	prepared.apply_root_finder_info(&mut root_info);
	assert_eq!(
		u16::from_be_bytes(root_info[ROOT_FINDER_FLAGS].try_into().unwrap()),
		0x2020 | CUSTOM_ICON
	);

	build.seal().unwrap().publish(None).unwrap();
	let (mut image, header) = open_output(&temp.path().join(IMAGE));
	assert_eq!(header.finder_info, [0; 32]);
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let records = || {
		node.records()
			.map(|record| Record::decode(record, header.geometry).unwrap())
	};
	let flags = records()
		.find_map(|record| match record.value {
			Value::Folder { metadata, .. } if metadata.id == ROOT_ID => Some(u16::from_be_bytes(
				metadata.finder_info[ROOT_FINDER_FLAGS].try_into().unwrap(),
			)),
			_ => None,
		})
		.unwrap();
	assert_eq!(flags, CUSTOM_ICON);
	let data = records()
		.find_map(|record| match record.value {
			Value::File { data, .. } if record.key.name.units().eq(ICON_ASSET.encode_utf16()) => {
				Some(data)
			}
			_ => None,
		})
		.unwrap();
	assert_eq!(data.logical_bytes, ICON_BYTES.len() as u64);
	let mut actual = vec![0; ICON_BYTES.len()];
	image
		.read_exact_at(fork_start(&data, header.geometry), &mut actual)
		.unwrap();
	assert_eq!(actual, ICON_BYTES);
}

#[test]
fn protects_the_icon_source_from_output_alias_and_staged_replacement() {
	let temp = tempfile::tempdir().unwrap();
	let source = temp.path().join(ICON_SOURCE);
	let output = temp.path().join(IMAGE);
	fs::write(&source, ICON_BYTES).unwrap();
	fs::hard_link(&source, &output).unwrap();
	let project = project("[build]\noverwrite = true\n");
	let build = || {
		hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		)
	};
	assert!(matches!(
		build(),
		Err(Error::Prepare(planning::Error::Manifest(
			manifest::Error::OutputSourceAlias { .. }
		)))
	));
	assert_eq!(fs::read(&output).unwrap(), ICON_BYTES);

	fs::remove_file(&output).unwrap();
	const PREVIOUS_IMAGE: &[u8] = b"previous published bytes";
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let prepared = build().unwrap();
	let sealed = prepared.seal().unwrap();
	let result = sealed.publish_with_progress(None, |progress| {
		if matches!(progress, BuildProgress::Publishing) {
			fs::write(&source, b"changed icon during publication").unwrap();
		}
	});
	assert!(matches!(
		result,
		Err(Error::Execute(ExecuteError::Replace(_)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	assert_eq!(
		fs::read_dir(temp.path())
			.unwrap()
			.filter_map(Result::ok)
			.filter(|entry| { entry.file_name().to_string_lossy().ends_with(".dmg") })
			.count(),
		1
	);
}

#[test]
fn counts_the_icon_against_entry_and_decoder_memory_budgets() {
	let temp = tempfile::tempdir().unwrap();
	fs::write(temp.path().join(ICON_SOURCE), ICON_BYTES).unwrap();
	for (project, expected_entry_error) in [
		(project("[performance]\nmax_entries = 1\n"), true),
		(project("[performance]\nmemory_limit = '32MiB'\n"), false),
	] {
		let error = hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		)
		.err()
		.unwrap();
		if expected_entry_error {
			assert!(matches!(
				error,
				Error::Prepare(planning::Error::Presentation(
					presentation::Error::EntryBudget
				))
			));
		} else {
			assert!(matches!(
				error,
				Error::Prepare(planning::Error::Presentation(
					presentation::Error::VolumeIconMemory
				))
			));
		}
	}
	assert!(!temp.path().join(IMAGE).exists());
}

#[test]
fn rejects_linked_and_oversized_icon_paths_before_encoding() {
	let temp = tempfile::tempdir().unwrap();
	let original = temp.path().join("original.icns");
	let source = temp.path().join(ICON_SOURCE);
	fs::write(&original, ICON_BYTES).unwrap();
	std::os::unix::fs::symlink(&original, &source).unwrap();
	let project = project("");
	let prepare = || {
		hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		)
	};
	assert!(matches!(
		prepare(),
		Err(Error::Prepare(planning::Error::Manifest(
			manifest::Error::RegularSourceRequired { .. }
		)))
	));
	fs::remove_file(&source).unwrap();
	const OVER_ICON_LIMIT_BYTES: u64 = (16 << 20) + 1;
	fs::File::create(&source)
		.unwrap()
		.set_len(OVER_ICON_LIMIT_BYTES)
		.unwrap();
	assert!(matches!(
		prepare(),
		Err(Error::Prepare(planning::Error::Presentation(
			presentation::Error::VolumeIcon(_)
		)))
	));
	assert!(!temp.path().join(IMAGE).exists());
}
