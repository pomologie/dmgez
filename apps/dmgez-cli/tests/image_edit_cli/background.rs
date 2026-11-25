use std::{collections::BTreeMap, fs, num::NonZeroUsize, path::Path};

use dmgez_builder::preflight::{Availability, OutputIntent, PreflightOptions, ReleaseIntent};
use dmgez_config::Project;
use dmgez_finder::{
	ImageBackgroundIdentity, STORE_NAME,
	inspection::{
		BackgroundAlias, BackgroundAliasName, BackgroundAliasTarget, HfsCreationTime, ReadLimits,
		Record, RecordValue, inspect,
	},
	write_store_with_background_identity,
};
use dmgez_hfsplus::browse::{Limits, with_catalog};
use dmgez_udif::reader::Image;
use dmgez_vfs::{Disk, VolumeId, volume::ForkKind};

use super::finder::{build_project, fork};
use super::{CHANGES, EDITED, ORIGINAL, assert_failure, edit, rebuild::json};

const PROJECT: &str = "background.toml";
const SOURCE: &str = "payload.txt";
const ORIGINAL_VOLUME: &str = "Cafe\u{301} Display";
const REBUILT_VOLUME: &str = "Muse\u{301}e Display";
const ROOT_STORE: &str = "/.DS_Store";
const EXHIBITS_STORE: &str = "/Exhibits/.DS_Store";
const MOVED_STORE: &str = "/Display/Gallery/.DS_Store";
const ORIGINAL_ASSET: &str = "/.background/background-0.tiff";
const REBUILT_ASSET: &str = "/Display/Gallery/Mura\u{301}l.tiff";
const PAYLOAD: &[u8] = b"synthetic displayed file\0\xff";
const NO_CHANGE: &str = "schema_version = 1\nedits = []\n";
const CREATED_DATE: usize = 0;
const REPLACEMENT: &str = "lookalike.tiff";
const REPORT_POLICY: [&str; 2] = ["--metadata-policy", "report"];
const SUPPLIED_DIR: &str = "supplied";
const SUPPLIED_PNG: &str = "Poster.png";
const SUPPLIED_ASSET: &str = "/Supplied/Poster.png";
const SUPPLIED_STORE: &str = "/Supplied/.DS_Store";
const SUPPLIED_MOVED_ASSET: &str = "/Supplied/Reframed.png";
const WRONG_SPELLING: &str = "/SUPPLIED/Poster.png";
const SUPPLIED_VOLUME: &str = "Supplied Background";
const PNG: &[u8] = include_bytes!(
	"../../../../tests/compatibility/project-build/tests/fixtures/background-root.png"
);
#[cfg(target_os = "macos")]
const RESOURCE_FORK: &[u8] = b"background resource fork\0\xff";

const TEMPLATE: &str = r#"
schema_version = 1
[image]
name = "Café Display"
output = "original.dmg"
format = "udzo"
size = "8MiB"
[build]
reproducible = true
source_date_epoch = 946684800
[metadata]
policy = "report"
preserve_timestamps = false
[[contents]]
source = "payload.txt"
destination = "/Card.txt"
position = [101, 102]
[[contents]]
source = "payload.txt"
destination = "/Exhibits/Details.txt"
position = [201, 202]
[[contents]]
type = "directory"
destination = "/Display"
[finder]
background = { kind = "template", name = "builtin-arrow" }
window_size = [620, 270]
show_status_bar = true
[finder.icon_view]
icon_size = 72
[[finder.folders]]
path = "/Exhibits"
window_size = [480, 330]
"#;

#[test]
fn preserves_original_background_stores_byte_for_byte_when_alias_ids_are_stable() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	let original = build_template(root);
	let identity = catalog_identity(&original, ORIGINAL_ASSET, ORIGINAL_VOLUME);
	let rebuilt = publish_edit(root, NO_CHANGE, &[]);
	assert_eq!(
		catalog_identity(&rebuilt, ORIGINAL_ASSET, ORIGINAL_VOLUME),
		identity
	);
	for path in [ROOT_STORE, EXHIBITS_STORE] {
		let layout = layout_at(&original, path);
		assert_alias(&layout.alias, &identity);
		assert_eq!(fork(&rebuilt, path, ForkKind::Data).unwrap(), layout.bytes);
	}
	assert_eq!(
		fork(&rebuilt, ORIGINAL_ASSET, ForkKind::Data).unwrap(),
		fork(&original, ORIGINAL_ASSET, ForkKind::Data).unwrap()
	);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
}

#[test]
fn rebinds_inherited_and_root_backgrounds_to_the_moved_catalog_identity() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	let original = build_template(root);
	let identity = catalog_identity(&original, ORIGINAL_ASSET, ORIGINAL_VOLUME);
	let before_root = layout_at(&original, ROOT_STORE);
	let before_nested = layout_at(&original, EXHIBITS_STORE);
	assert_alias(&before_root.alias, &identity);
	assert_alias(&before_nested.alias, &identity);
	assert_ne!(before_root.settings, before_nested.settings);
	assert_positions(&before_root, "Card.txt", [101, 102]);
	assert_positions(&before_nested, "Details.txt", [201, 202]);

	let artwork = fork(&original, ORIGINAL_ASSET, ForkKind::Data).unwrap();
	let batch = format!(
		r#"
schema_version = 1
[[edits]]
operation = "add_directory"
destination = "/!Earlier"
[[edits]]
operation = "modify"
target = "/Exhibits"
destination = "/DISPLAY/Gallery"
[[edits]]
operation = "modify"
target = "{ORIGINAL_ASSET}"
destination = "/display/GALLERY/Murál.tiff"
[[edits]]
operation = "modify"
target = "/Card.txt"
destination = "/Poster.txt"
position = [315, 316]
[[edits]]
operation = "set_image_name"
name = "Musée Display"
"#
	);
	let rebuilt = publish_edit(root, &batch, &[]);
	let rebound = catalog_identity(&rebuilt, REBUILT_ASSET, REBUILT_VOLUME);
	assert_ne!(identity.target_id, rebound.target_id);
	assert_ne!(identity.parent_id, rebound.parent_id);
	assert_eq!(identity.target_created, rebound.target_created);
	assert_eq!(identity.volume_created, rebound.volume_created);
	let after_root = layout_at(&rebuilt, ROOT_STORE);
	let after_nested = layout_at(&rebuilt, MOVED_STORE);
	assert_alias(&after_root.alias, &rebound);
	assert_alias(&after_nested.alias, &rebound);
	assert_eq!(before_root.settings, after_root.settings);
	assert_eq!(before_nested.settings, after_nested.settings);
	assert_positions(&after_root, "Poster.txt", [315, 316]);
	assert_positions(&after_nested, "Details.txt", [201, 202]);
	assert_eq!(
		fork(&rebuilt, REBUILT_ASSET, ForkKind::Data).unwrap(),
		artwork
	);
	assert!(fork(&rebuilt, ORIGINAL_ASSET, ForkKind::Data).is_none());
	assert!(fork(&rebuilt, EXHIBITS_STORE, ForkKind::Data).is_none());
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	#[cfg(target_os = "macos")]
	{
		let output = std::process::Command::new("/usr/bin/hdiutil")
			.current_dir(root)
			.args(["verify", EDITED])
			.output()
			.unwrap();
		assert!(output.status.success(), "{output:?}");
	}
}

#[test]
fn refuses_to_remove_or_replace_a_surviving_stores_original_target() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	let original = build_supplied(root, |_| {});
	fs::write(root.join(REPLACEMENT), PNG).unwrap();
	let remove = format!(
		r#"
schema_version = 1
[[edits]]
operation = "remove"
target = "{SUPPLIED_ASSET}"
"#
	);
	let replace = format!(
		r#"{remove}
[[edits]]
operation = "add_copy"
source = "{REPLACEMENT}"
destination = "{SUPPLIED_ASSET}"
"#
	);
	for batch in [&remove, &replace] {
		fs::write(root.join(CHANGES), batch).unwrap();
		assert_failure(
			edit(root, EDITED, &REPORT_POLICY),
			"Finder background target would not survive the edit",
		);
		assert!(!root.join(EDITED).exists());
		assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	}
}

#[test]
fn rebinds_a_supplied_alias_and_preserves_its_targets_forks() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path();
	let original = build_supplied(root, |_| {});
	let before = catalog_identity(&original, SUPPLIED_ASSET, SUPPLIED_VOLUME);
	assert_alias(&layout_at(&original, SUPPLIED_STORE).alias, &before);
	fs::write(root.join(REPLACEMENT), PAYLOAD).unwrap();
	#[cfg(target_os = "macos")]
	assert_eq!(
		fork(&original, SUPPLIED_ASSET, ForkKind::Resource).unwrap(),
		RESOURCE_FORK
	);
	let batch = format!(
		r#"
schema_version = 1
[[edits]]
operation = "modify"
target = "{SUPPLIED_ASSET}"
destination = "{SUPPLIED_MOVED_ASSET}"
[[edits]]
operation = "add_copy"
source = "{REPLACEMENT}"
destination = "{SUPPLIED_ASSET}"
"#
	);
	let rebuilt = publish_edit(root, &batch, &REPORT_POLICY);
	let rebound = catalog_identity(&rebuilt, SUPPLIED_MOVED_ASSET, SUPPLIED_VOLUME);
	assert_alias(&layout_at(&rebuilt, SUPPLIED_STORE).alias, &rebound);
	assert_eq!(
		fork(&rebuilt, SUPPLIED_MOVED_ASSET, ForkKind::Data).unwrap(),
		PNG
	);
	assert_eq!(
		fork(&rebuilt, SUPPLIED_ASSET, ForkKind::Data).unwrap(),
		PAYLOAD
	);
	#[cfg(target_os = "macos")]
	assert_eq!(
		fork(&rebuilt, SUPPLIED_MOVED_ASSET, ForkKind::Resource).unwrap(),
		RESOURCE_FORK
	);
	assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
}

#[test]
fn rejects_canonical_aliases_that_disagree_with_the_original_catalog() {
	type ModifyAlias = fn(&mut ImageBackgroundIdentity<'_>);
	const EXACT_PATH: &str = "does not name an exact ordinary file in the source catalog";
	const TARGET_IDENTITY: &str = "does not match the target catalog IDs or creation date";
	const VOLUME_IDENTITY: &str = "does not match the source volume identity";
	let cases: [(ModifyAlias, &str); 5] = [
		(
			|alias| alias.volume_relative_path = WRONG_SPELLING,
			EXACT_PATH,
		),
		(|alias| alias.target_catalog_id += 1, TARGET_IDENTITY),
		(
			|alias| alias.parent_catalog_id = alias.target_catalog_id + 1,
			TARGET_IDENTITY,
		),
		(
			|alias| alias.target_created_hfs_seconds += 1,
			TARGET_IDENTITY,
		),
		(
			|alias| alias.volume_created_hfs_seconds += 1,
			VOLUME_IDENTITY,
		),
	];
	for (modify_alias, diagnostic) in cases {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		let original = build_supplied(root, modify_alias);
		fs::write(root.join(CHANGES), NO_CHANGE).unwrap();
		assert_failure(edit(root, EDITED, &[]), diagnostic);
		assert!(!root.join(EDITED).exists());
		assert_eq!(fs::read(root.join(ORIGINAL)).unwrap(), original);
	}
}

fn build_template(root: &Path) -> Vec<u8> {
	fs::write(root.join(SOURCE), PAYLOAD).unwrap();
	fs::write(root.join(PROJECT), TEMPLATE).unwrap();
	build_project(root, PROJECT)
}

fn build_supplied(
	root: &Path,
	modify_alias: impl FnOnce(&mut ImageBackgroundIdentity<'_>),
) -> Vec<u8> {
	const SEED: &str = "seed.dmg";
	let source = root.join(SUPPLIED_DIR);
	fs::create_dir(&source).unwrap();
	let asset = source.join(SUPPLIED_PNG);
	fs::write(&asset, PNG).unwrap();
	#[cfg(target_os = "macos")]
	fs::write(asset.join("..namedfork/rsrc"), RESOURCE_FORK).unwrap();
	let store_path = source.join(STORE_NAME);
	// A placeholder supplies the same catalog entry as the final store.
	fs::write(&store_path, []).unwrap();
	let seed = publish_supplied(root, SEED);
	let identity = catalog_identity(&seed, SUPPLIED_ASSET, SUPPLIED_VOLUME);
	let project = Project::parse(TEMPLATE).unwrap();
	let options = &project.finder.as_ref().unwrap().options;
	let mut alias = identity.as_background();
	modify_alias(&mut alias);
	let bytes = write_store_with_background_identity(options, &[], alias).unwrap();
	fs::write(store_path, bytes).unwrap();
	let original = publish_supplied(root, ORIGINAL);
	assert_eq!(
		catalog_identity(&original, SUPPLIED_ASSET, SUPPLIED_VOLUME),
		identity
	);
	original
}

fn publish_supplied(root: &Path, output: &str) -> Vec<u8> {
	let project = Project::parse(&format!(
		r#"
schema_version = 1
[image]
name = "{SUPPLIED_VOLUME}"
output = "{output}"
format = "udzo"
size = "4MiB"
[build]
reproducible = true
source_date_epoch = 946684800
[metadata]
policy = "report"
preserve_timestamps = false
[[contents]]
source = "{SUPPLIED_DIR}"
destination = "/Supplied"
"#
	))
	.unwrap();
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.finder_layout = Availability::Unavailable;
	dmgez_hfsplus::prepare(
		&project,
		&root.canonicalize().unwrap(),
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
		Default::default(),
	)
	.unwrap()
	.seal()
	.unwrap()
	.publish(None)
	.unwrap();
	fs::read(root.join(output)).unwrap()
}

fn publish_edit(root: &Path, batch: &str, args: &[&str]) -> Vec<u8> {
	fs::write(root.join(CHANGES), batch).unwrap();
	let report = json(edit(root, EDITED, &[args, &["--json"]].concat()));
	assert_eq!(report["published"], true);
	assert_eq!(report["verified"], true);
	assert_eq!(fs::read_to_string(root.join(CHANGES)).unwrap(), batch);
	fs::read(root.join(EDITED)).unwrap()
}

#[derive(Debug, Eq, PartialEq)]
struct CatalogIdentity {
	volume_name: String,
	volume_created: u32,
	target_created: u32,
	parent_id: u32,
	target_id: u32,
	path: String,
}

impl CatalogIdentity {
	fn as_background(&self) -> ImageBackgroundIdentity<'_> {
		ImageBackgroundIdentity {
			volume_name: &self.volume_name,
			volume_created_hfs_seconds: self.volume_created,
			target_created_hfs_seconds: self.target_created,
			parent_catalog_id: self.parent_id,
			target_catalog_id: self.target_id,
			volume_relative_path: &self.path,
		}
	}
}

fn catalog_identity(bytes: &[u8], expected_path: &str, expected_volume: &str) -> CatalogIdentity {
	let image = Image::open(bytes, dmgez_udif::reader::Limits::default()).unwrap();
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).unwrap();
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		Limits::default(),
		|volume, catalog| {
			let mut entry = catalog.root().unwrap();
			let volume_name = String::from_utf16(&entry.name).unwrap();
			assert_eq!(volume_name, expected_volume);
			let mut path = String::new();
			for component in expected_path.strip_prefix('/').unwrap().split('/') {
				entry = catalog
					.lookup(entry.metadata.id, component)
					.unwrap()
					.unwrap();
				path.push('/');
				path.push_str(&String::from_utf16(&entry.name).unwrap());
			}
			assert_eq!(path, expected_path);
			let file = catalog
				.regular_file(entry.metadata.id, Limits::default().hard_links)
				.unwrap()
				.unwrap();
			CatalogIdentity {
				volume_name,
				volume_created: volume.header().dates[CREATED_DATE],
				target_created: file.metadata().dates[CREATED_DATE],
				parent_id: entry.parent.0,
				target_id: entry.metadata.id.0,
				path,
			}
		},
	)
	.unwrap()
}

fn assert_alias(alias: &BackgroundAliasTarget, identity: &CatalogIdentity) {
	let (_, filename) = identity.path.rsplit_once('/').unwrap();
	assert_eq!(
		alias.volume_name,
		Some(BackgroundAliasName::Unicode(identity.volume_name.clone()))
	);
	assert_eq!(
		alias.target_name,
		Some(BackgroundAliasName::Unicode(filename.to_owned()))
	);
	assert_eq!(alias.target_path.as_deref(), Some(identity.path.as_bytes()));
	assert_eq!(alias.parent_catalog_id, identity.parent_id);
	assert_eq!(alias.target_catalog_id, identity.target_id);
	assert_eq!(
		alias.volume_created,
		HfsCreationTime::Seconds(identity.volume_created)
	);
	assert_eq!(
		alias.target_created,
		HfsCreationTime::Seconds(identity.target_created)
	);
}

struct StoreLayout {
	bytes: Vec<u8>,
	alias: BackgroundAliasTarget,
	settings: Vec<Record>,
	positions: BTreeMap<String, [u32; 2]>,
}

fn layout_at(image: &[u8], path: &str) -> StoreLayout {
	let bytes = fork(image, path, ForkKind::Data).unwrap();
	let mut alias = None;
	let mut settings = Vec::new();
	let mut positions = BTreeMap::new();
	for mut record in inspect(&bytes, ReadLimits::default()).unwrap().records {
		if let RecordValue::IconPosition { x, y } = record.value {
			assert!(positions.insert(record.name, [x, y]).is_none());
		} else {
			if let RecordValue::IconView(icons) = &mut record.value {
				let Some(BackgroundAlias::File(target)) = icons.background_alias.take() else {
					panic!("expected a decoded background alias at {path}");
				};
				assert!(alias.replace(*target).is_none());
			}
			settings.push(record);
		}
	}
	StoreLayout {
		bytes,
		alias: alias.unwrap(),
		settings,
		positions,
	}
}

fn assert_positions(layout: &StoreLayout, name: &str, coordinates: [u32; 2]) {
	assert_eq!(
		layout.positions,
		BTreeMap::from([(name.to_owned(), coordinates)])
	);
}
