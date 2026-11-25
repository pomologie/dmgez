use dmgez_builder::{planning, preflight::PreflightError};
use dmgez_config::{ConfigError, ContentError};

use super::{
	Asset, BACKGROUND_FOLDER, CREATED_DATE, Command, EXPECTED_HFS_DATE, Error, FileSource, Header,
	IMAGE, Image, Node, OVERRIDE_BYTES, OVERRIDE_SOURCE, OutputIntent, Path, Project, ROOT_ID,
	Stdio, Value, Write, assert_fixture_aliases, assert_invisible, catalog_records, first_leaf,
	folder, fs, hfsplus, open_output, options, publish_project, read_fork,
};

const PROJECT: &str = include_str!("../fixtures/finder-background.toml");
const ROOT_IMAGE: &str = r#"kind = "image", source = "background-root.png""#;
const ARROW_TEMPLATE: &str = "builtin-arrow";
const UNKNOWN_TEMPLATE: &str = "unknown-template";
const ARROW_ASSET: &str = "background-0.tiff";
const PREVIOUS_IMAGE: &[u8] = b"published image before template change";
const ARROW_ORACLE: &str = concat!(
	env!("CARGO_MANIFEST_DIR"),
	"/tests/smoke/template_background_oracle.py"
);

fn project(name: &str) -> Project {
	Project::parse(&settings(name)).unwrap()
}

fn settings(name: &str) -> String {
	let template = format!(r#"kind = "template", name = "{name}""#);
	assert_eq!(PROJECT.matches(ROOT_IMAGE).count(), 1);
	PROJECT
		.replace(ROOT_IMAGE, &template)
		.replace(r#"size = "1MiB""#, r#"size = "8MiB""#)
}

fn publish_template(root: &Path) {
	fs::write(root.join(OVERRIDE_SOURCE), OVERRIDE_BYTES).unwrap();
	publish_project(root, &project(ARROW_TEMPLATE));
}

fn read_assets(
	image: &mut Image<FileSource>,
	node: &Node<'_>,
	header: &Header,
) -> ([Asset; 2], Vec<u8>) {
	let background = folder(node, header, ROOT_ID, BACKGROUND_FOLDER);
	assert_invisible(background);
	let mut template = None;
	let mut overridden = None;
	for record in catalog_records(node, header) {
		let Value::File { metadata, data, .. } = record.value else {
			continue;
		};
		if record.key.parent_or_thread != background.id {
			continue;
		}
		let name = String::from_utf16(&record.key.name.units().collect::<Vec<_>>()).unwrap();
		let is_template = name == ARROW_ASSET;
		assert_invisible(metadata);
		assert_eq!(metadata.dates[CREATED_DATE], EXPECTED_HFS_DATE);
		let bytes = read_fork(image, header, &data);
		let asset = Asset {
			name,
			metadata,
			parent: background.id,
		};
		if is_template {
			assert!(template.replace((asset, bytes)).is_none());
		} else {
			assert_eq!(bytes, OVERRIDE_BYTES, "unexpected generated background");
			assert!(overridden.replace(asset).is_none());
		}
	}
	let (template, bytes) = template.unwrap();
	let overridden = overridden.unwrap();
	assert_ne!(template.metadata.id, overridden.metadata.id);
	([template, overridden], bytes)
}

#[test]
fn generates_one_hidden_arrow_for_the_root_and_inherited_layout() {
	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	for root in [first.path(), second.path()] {
		publish_template(root);
		assert_eq!(fs::read_dir(root).unwrap().count(), 2);
	}
	let output = first.path().join(IMAGE);
	assert_eq!(
		fs::read(&output).unwrap(),
		fs::read(second.path().join(IMAGE)).unwrap()
	);
	let (mut image, header) = open_output(&output);
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let (_, pixels) = read_assets(&mut image, &node, &header);
	let mut oracle = Command::new("python3")
		.args(["-B", ARROW_ORACLE])
		.stdin(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(&pixels).unwrap();
	assert!(oracle.wait().unwrap().success());
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independently_matches_template_aliases_to_the_frozen_catalog_identity() {
	let temp = tempfile::tempdir().unwrap();
	publish_template(temp.path());
	let (mut image, header) = open_output(&temp.path().join(IMAGE));
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let (assets, _) = read_assets(&mut image, &node, &header);
	assert_fixture_aliases(&mut image, &node, &header, &assets);
}

#[test]
fn rejects_an_unknown_template_before_opening_sources_or_replacing_output() {
	let temp = tempfile::tempdir().unwrap();
	let output = temp.path().join(IMAGE);
	fs::write(&output, PREVIOUS_IMAGE).unwrap();
	let project =
		Project::parse(&settings(UNKNOWN_TEMPLATE).replace("[build]", "[build]\noverwrite = true"))
			.unwrap();
	let Err(Error::Prepare(planning::Error::Preflight(PreflightError::Finder(error)))) =
		hfsplus::prepare(
			&project,
			temp.path(),
			options(OutputIntent::Path),
			Default::default(),
		)
	else {
		panic!("expected the template to fail before source discovery");
	};
	assert!(error.to_string().contains("template"), "{error}");
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS_IMAGE);
}

#[test]
fn refuses_a_collision_between_the_template_namespace_and_project_content() {
	let contents = format!(
		"[[contents]]\ntype = 'directory'\ndestination = '/{BACKGROUND_FOLDER}'\n\n[finder]"
	);
	assert!(matches!(
		Project::parse(&settings(ARROW_TEMPLATE).replace("[finder]", &contents)),
		Err(ConfigError::Content {
			error: ContentError::ReservedDestination,
			..
		})
	));
}
