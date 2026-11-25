use std::{
	io::Write,
	process::{Command, Stdio},
};

use dmgez_builder::{planning, presentation, progress::BuildProgress};
use hfsplus_format::{catalog::ThreadKind, volume::CatalogId};

use super::*;

const ICON_SOURCE: &str = "source.icns";
const ICON_ASSET: &str = ".VolumeIcon.icns";
const ICON_BYTES: &[u8] = include_bytes!("apple-iconutil.icns");
const ROOT_ID: CatalogId = CatalogId(2);
const ROOT_FINDER_FLAGS: std::ops::Range<usize> = 8..10;
const CUSTOM_ICON: u16 = 0x0400;
const IMAGE_SETTINGS: &str = "[volume_icon.images]";
const ICON_FIXTURE: &str = include_str!("volume_icon_fixture.py");
const SMALLEST_PNG: &str = "icon_16x16.png";
const LARGEST_PNG: &str = "icon_512x512@2x.png";
const REPRODUCIBLE: &str = "[build]\nreproducible = true\nsource_date_epoch = 946684800\n";
const OVERWRITE: &str = "[build]\noverwrite = true\n";
const PREVIOUS_IMAGE: &[u8] = b"previous published bytes";
const PNG_IMAGE_DATA: &[u8] = b"IDAT";
const LIMITED_ICON_METADATA: &str = "[performance]\nmax_metadata_bytes = '16KiB'\n";

fn project(settings: &str) -> Project {
	Project::parse(&format!(
		"{PROJECT_HEADER}[volume_icon]\nsource = '{ICON_SOURCE}'\n{settings}"
	))
	.unwrap()
}

fn images_project(settings: &str, fields: &str) -> Project {
	Project::parse(&format!(
		"{PROJECT_HEADER}{settings}{IMAGE_SETTINGS}\n{fields}"
	))
	.unwrap()
}

fn write_png_sources(root: &Path) -> String {
	let generation = Command::new("python3")
		.args(["-B", "-c", ICON_FIXTURE])
		.arg(root)
		.output()
		.unwrap();
	assert!(
		generation.status.success(),
		"{}",
		String::from_utf8_lossy(&generation.stderr)
	);
	String::from_utf8(generation.stdout).unwrap()
}

fn verify_icon_elements(root: &Path, icon: &[u8]) {
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ICON_FIXTURE, "--verify"])
		.arg(root)
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(icon).unwrap();
	assert!(oracle.wait().unwrap().success());
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
	assert_eq!(read_published_icon(&temp.path().join(IMAGE)), ICON_BYTES);
}

fn read_published_icon(path: &Path) -> Vec<u8> {
	let (mut image, header) = open_output(path);
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
	let (identity, data) = records()
		.find_map(|record| match record.value {
			Value::File { metadata, data, .. }
				if record.key.parent_or_thread == ROOT_ID
					&& record.key.name.units().eq(ICON_ASSET.encode_utf16()) =>
			{
				Some((metadata.id, data))
			}
			_ => None,
		})
		.unwrap();
	assert_ne!(identity, ROOT_ID);
	assert!(records().any(|record| {
		record.key.parent_or_thread == identity
			&& matches!(
				record.value,
				Value::Thread { kind: ThreadKind::File, parent, name }
					if parent == ROOT_ID && name.units().eq(ICON_ASSET.encode_utf16())
			)
	}));
	let mut actual = vec![0; usize::try_from(data.logical_bytes).unwrap()];
	image
		.read_exact_at(fork_start(&data, header.geometry), &mut actual)
		.unwrap();
	actual
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
fn builds_ten_icon_slots_with_stable_catalog_and_wire_bytes() {
	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	let fields = write_png_sources(first.path());
	assert_eq!(write_png_sources(second.path()), fields);
	let reversed = fields.lines().rev().collect::<Vec<_>>().join("\n");

	for (root, entries) in [(first.path(), &fields), (second.path(), &reversed)] {
		let project = images_project(REPRODUCIBLE, entries);
		let build = hfsplus::prepare(
			&project,
			root,
			options(OutputIntent::Path),
			Default::default(),
		)
		.unwrap();
		assert!(!root.join(IMAGE).exists());
		assert!(build.prepared().manifest().entries().is_empty());
		assert_eq!(build.prepared().finder_assets().len(), 2);
		build.seal().unwrap().publish(None).unwrap();
	}
	let output = first.path().join(IMAGE);
	assert_eq!(
		fs::read(&output).unwrap(),
		fs::read(second.path().join(IMAGE)).unwrap()
	);
	verify_icon_elements(first.path(), &read_published_icon(&output));
}

#[test]
fn rejects_wrong_pixel_dimensions_and_corrupt_image_slots_before_publishing() {
	let temp = tempfile::tempdir().unwrap();
	let fields = write_png_sources(temp.path());
	let project = images_project("", &fields);
	let root = temp.path();
	let retina = root.join(LARGEST_PNG);
	let original = fs::read(&retina).unwrap();
	let wrong_dimensions = fs::read(root.join(SMALLEST_PNG)).unwrap();
	let mut bad_crc = original.clone();
	let position = bad_crc
		.windows(PNG_IMAGE_DATA.len())
		.position(|bytes| bytes == PNG_IMAGE_DATA)
		.unwrap()
		+ PNG_IMAGE_DATA.len();
	bad_crc[position] ^= 1;
	for (bad_png, expected_message) in [
		(wrong_dimensions.as_slice(), "dimensions"),
		(&original[..original.len() / 2], "PNG"),
		(&bad_crc, "checksum"),
	] {
		fs::write(&retina, bad_png).unwrap();
		let Err(Error::Prepare(planning::Error::Presentation(presentation::Error::VolumeIcon(
			error,
		)))) = hfsplus::prepare(
			&project,
			root,
			options(OutputIntent::Path),
			Default::default(),
		)
		else {
			panic!("expected an invalid configured PNG to fail in presentation planning");
		};
		assert!(
			error.to_string().contains(expected_message),
			"unexpected icon diagnostic: {error}"
		);
		assert!(!root.join(IMAGE).exists());
	}
}

#[test]
fn charges_the_generated_icns_to_the_metadata_budget() {
	let temp = tempfile::tempdir().unwrap();
	let fields = write_png_sources(temp.path());
	let project = images_project(LIMITED_ICON_METADATA, &fields);
	assert!(matches!(
		hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		),
		Err(Error::Prepare(planning::Error::Presentation(
			presentation::Error::MetadataBudget
		)))
	));
	assert!(!temp.path().join(IMAGE).exists());
}

#[test]
fn protects_every_icon_source_from_output_alias_and_staged_replacement() {
	let temp = tempfile::tempdir().unwrap();
	let fields = write_png_sources(temp.path());
	let root = temp.path();
	let source = root.join(LARGEST_PNG);
	let output = root.join(IMAGE);
	let project = images_project(OVERWRITE, &fields);
	let prepare = || {
		hfsplus::prepare(
			&project,
			root,
			options(OutputIntent::Path),
			Default::default(),
		)
	};
	let sources = fs::read_dir(root)
		.unwrap()
		.map(|entry| entry.unwrap().path())
		.collect::<Vec<_>>();
	assert_eq!(sources.len(), fields.lines().count());
	for input in sources {
		let original = fs::read(&input).unwrap();
		fs::hard_link(&input, &output).unwrap();
		assert!(matches!(
			prepare(),
			Err(Error::Prepare(planning::Error::Manifest(
				manifest::Error::OutputSourceAlias { .. }
			)))
		));
		assert_eq!(fs::read(&output).unwrap(), original);
		fs::remove_file(&output).unwrap();
	}

	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let prepared = prepare().unwrap();
	let result = prepared
		.seal()
		.unwrap()
		.publish_with_progress(None, |progress| {
			if matches!(progress, BuildProgress::Publishing) {
				fs::write(&source, b"changed icon slot during publication").unwrap();
			}
		});
	assert!(matches!(
		result,
		Err(Error::Execute(ExecuteError::Replace(_)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	assert_eq!(
		fs::read_dir(root).unwrap().count(),
		fields.lines().count() + 1
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
