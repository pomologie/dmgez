use dmgez_builder::{manifest, planning, presentation};

use super::{
	Asset, BACKGROUND_FOLDER, BuildProgress, CREATED_DATE, Command, EXPECTED_HFS_DATE, Error,
	ExecuteError, FileSource, Header, IMAGE, Image, Node, OutputIntent, Path, Project, ROOT_BYTES,
	ROOT_ID, ROOT_SOURCE, Stdio, Value, Write, assert_fixture_aliases, assert_invisible,
	catalog_records, first_leaf, folder, fs, hfsplus, open_output, options, publish_project,
	read_fork,
};

const PROJECT: &str = include_str!("../fixtures/finder-retina-background.toml");
const RETINA_SOURCE: &str = "background-root@2x.png";
const RETINA_BYTES: &[u8] = include_bytes!("../fixtures/background-root@2x.png");
const PREBUILT_SOURCE: &str = "prebuilt.image";
const RETINA_ASSET: &str = "background-0.tiff";
const STATIC_ASSET: &str = "background-1.png";
const PREVIOUS_IMAGE: &[u8] = b"previous published image";
const TIFF_ORACLE: &str = include_str!("retina_background_oracle.py");

fn write_sources(root: &Path) {
	fs::write(root.join(ROOT_SOURCE), ROOT_BYTES).unwrap();
	fs::write(root.join(RETINA_SOURCE), RETINA_BYTES).unwrap();
}

fn assert_assets(
	image: &mut Image<FileSource>,
	node: &Node<'_>,
	header: &Header,
) -> ([Asset; 2], Vec<u8>) {
	let background = folder(node, header, ROOT_ID, BACKGROUND_FOLDER);
	assert_invisible(background);
	let mut retina = None;
	let mut static_image = None;
	for record in catalog_records(node, header) {
		let Value::File { metadata, data, .. } = record.value else {
			continue;
		};
		if record.key.parent_or_thread != background.id {
			continue;
		}
		assert_invisible(metadata);
		assert_eq!(metadata.dates[CREATED_DATE], EXPECTED_HFS_DATE);
		let asset = Asset {
			name: String::from_utf16(&record.key.name.units().collect::<Vec<_>>()).unwrap(),
			metadata,
			parent: background.id,
		};
		let bytes = read_fork(image, header, &data);
		match asset.name.as_str() {
			RETINA_ASSET => {
				assert!(retina.is_none(), "duplicated inherited Retina asset");
				retina = Some((asset, bytes));
			}
			STATIC_ASSET => {
				assert!(static_image.is_none(), "duplicated static override");
				assert_eq!(bytes, ROOT_BYTES);
				static_image = Some(asset);
			}
			name => panic!("unexpected background asset {name}"),
		}
	}
	let (retina, tiff) = retina.unwrap();
	let static_image = static_image.unwrap();
	assert_ne!(retina.metadata.id, static_image.metadata.id);
	([retina, static_image], tiff)
}

fn read_published_tiff(path: &Path) -> Vec<u8> {
	let (mut image, header) = open_output(path);
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let (_, tiff) = assert_assets(&mut image, &node, &header);
	tiff
}

fn presentation_error(root: &Path, project: &Project) -> presentation::Error {
	let Err(Error::Prepare(planning::Error::Presentation(error))) = hfsplus::prepare(
		project,
		root,
		options(OutputIntent::Path),
		Default::default(),
	) else {
		panic!("expected invalid background to fail in presentation planning");
	};
	error
}

#[test]
fn stores_one_deterministic_retina_asset_and_keeps_the_static_override() {
	let project = Project::parse(PROJECT).unwrap();
	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	for directory in [&first, &second] {
		write_sources(directory.path());
		publish_project(directory.path(), &project);
	}
	let output = first.path().join(IMAGE);
	assert_eq!(
		fs::read(&output).unwrap(),
		fs::read(second.path().join(IMAGE)).unwrap()
	);
	let tiff = read_published_tiff(&output);
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", TIFF_ORACLE])
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(&tiff).unwrap();
	assert!(oracle.wait().unwrap().success());
}

#[test]
fn preserves_prebuilt_retina_tiff_bytes_and_rejects_truncated_data() {
	let paired = tempfile::tempdir().unwrap();
	write_sources(paired.path());
	publish_project(paired.path(), &Project::parse(PROJECT).unwrap());
	let tiff = read_published_tiff(&paired.path().join(IMAGE));

	let prebuilt = tempfile::tempdir().unwrap();
	fs::write(prebuilt.path().join(ROOT_SOURCE), ROOT_BYTES).unwrap();
	let source = prebuilt.path().join(PREBUILT_SOURCE);
	let output = prebuilt.path().join(IMAGE);
	let pair = format!("source = \"{ROOT_SOURCE}\", retina = \"{RETINA_SOURCE}\"");
	let single = format!("source = \"{PREBUILT_SOURCE}\"");
	let project = Project::parse(&PROJECT.replace(&pair, &single)).unwrap();

	fs::write(&source, &tiff[..tiff.len() / 2]).unwrap();
	assert!(matches!(
		presentation_error(prebuilt.path(), &project),
		presentation::Error::BackgroundTiff(_) | presentation::Error::BackgroundTiffRepresentation
	));
	assert!(!output.exists());

	fs::write(&source, &tiff).unwrap();
	publish_project(prebuilt.path(), &project);
	assert_eq!(read_published_tiff(&output), tiff);
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_oracle_matches_retina_alias_to_catalog_and_respects_overrides() {
	let temp = tempfile::tempdir().unwrap();
	write_sources(temp.path());
	publish_project(temp.path(), &Project::parse(PROJECT).unwrap());
	let (mut image, header) = open_output(&temp.path().join(IMAGE));
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let (assets, _) = assert_assets(&mut image, &node, &header);
	assert_fixture_aliases(&mut image, &node, &header, &assets);
}

#[test]
fn rejects_wrong_scale_and_malformed_retina_without_publishing() {
	let temp = tempfile::tempdir().unwrap();
	write_sources(temp.path());
	let project = Project::parse(PROJECT).unwrap();
	let source = temp.path().join(RETINA_SOURCE);
	let output = temp.path().join(IMAGE);
	fs::write(&source, ROOT_BYTES).unwrap();
	assert!(matches!(
		presentation_error(temp.path(), &project),
		presentation::Error::BackgroundRetinaDimensions
	));
	assert!(!output.exists());

	fs::write(&source, &RETINA_BYTES[..RETINA_BYTES.len() / 2]).unwrap();
	assert!(matches!(
		presentation_error(temp.path(), &project),
		presentation::Error::BackgroundPng(_)
	));
	assert!(!output.exists());
}

#[test]
fn protects_the_retina_source_from_output_alias_and_staged_replacement() {
	let temp = tempfile::tempdir().unwrap();
	write_sources(temp.path());
	let source = temp.path().join(RETINA_SOURCE);
	let output = temp.path().join(IMAGE);
	let project = Project::parse(&PROJECT.replace("[build]", "[build]\noverwrite = true")).unwrap();
	let prepare = || {
		hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		)
	};
	fs::hard_link(&source, &output).unwrap();
	assert!(matches!(
		presentation_error(temp.path(), &project),
		presentation::Error::BackgroundSource(manifest::Error::OutputSourceAlias { .. })
	));
	assert_eq!(fs::read(&output).unwrap(), RETINA_BYTES);

	fs::remove_file(&output).unwrap();
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let prepared = prepare().unwrap();
	let result = prepared
		.seal()
		.unwrap()
		.publish_with_progress(None, |progress| {
			if matches!(progress, BuildProgress::Publishing) {
				fs::write(&source, b"changed Retina image during publication").unwrap();
			}
		});
	assert!(matches!(
		result,
		Err(Error::Execute(ExecuteError::Replace(_)))
	));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
	assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
}
