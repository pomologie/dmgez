#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::{
	env, fs,
	path::Path,
	process::{Command, Output},
};

use dmgez_udif::reader::{FileSource, Image, Limits, RunEncoding};

const BINARY_ENV: &str = "DMGEZ_PROJECT_BUILD_CLI";
const CONFIG_DIRECTORY: &str = "project";
const CONFIG_PATH: &str = "project/dmg.toml";
const TOML_OUTPUT: &str = "from-config.dmg";
const CLI_OUTPUT: &str = "from-caller.dmg";
const CLI_OUTPUT_OVERRIDE: &str = "image.output=from-caller.dmg";
const CLI_SIZE_OVERRIDE: &str = "image.size=256KiB";
const CLI_FORMAT_OVERRIDE: &str = "image.format=udro";
const ULMO_FORMAT_OVERRIDE: &str = "image.format=ulmo";
const CLI_SIZE_BYTES: u64 = 256 * 1024;
const SOURCE_NAME: &str = "payload=literal.txt";
const SOURCE_DEFINE: &str = "CONTENTS=payload=literal.txt";
const CONTENTS: &[u8] = b"literal input payload";
const PREVIOUS: &[u8] = b"preexisting output";
const LIST_CONFIG: &str = "finder-list.toml";
const LIST_OUTPUT: &str = "list-layout.dmg";
const LIST_MANIFEST: &str = include_str!("fixtures/finder-list.toml");
const COLUMN_CONFIG: &str = "finder-column.toml";
const COLUMN_OUTPUT: &str = "column-layout.dmg";
const COLUMN_MANIFEST: &str = include_str!("fixtures/finder-column.toml");
const FINDER_FIXTURE_LOGICAL_BYTES: u64 = 1 << 20;
const MANIFEST: &str = r#"
schema_version = 1

[defines]
CONTENTS = 'missing-source.txt'

[image]
name = 'Developer build'
output = 'from-config.dmg'
size = '1MiB'

[metadata]
preserve_timestamps = false

[[contents]]
source = '${CONTENTS}'
"#;

fn project(root: &Path) {
	let config = root.join(CONFIG_DIRECTORY);
	fs::create_dir(&config).unwrap();
	fs::write(root.join(CONFIG_PATH), MANIFEST).unwrap();
	fs::write(config.join(SOURCE_NAME), CONTENTS).unwrap();
}

fn command(caller: &Path) -> Command {
	let executable = env::var_os(BINARY_ENV).expect("run command fixtures through run.py");
	let mut command = Command::new(executable);
	command.current_dir(caller).arg("build");
	command
}

fn run_project(caller: &Path, extra: &[&str]) -> Output {
	command(caller)
		.args([CONFIG_PATH, "--define", SOURCE_DEFINE])
		.args(extra)
		.output()
		.unwrap()
}

fn successful(output: Output) -> (String, String) {
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(output.status.success(), "{stderr}");
	(String::from_utf8(output.stdout).unwrap(), stderr)
}

#[test]
fn dry_run_reports_exact_capacity_and_resolves_each_path_from_its_origin() {
	let caller = tempfile::tempdir().unwrap();
	let root = caller.path().canonicalize().unwrap();
	project(&root);
	let overrides = [
		"--set",
		"image.output=obsolete.dmg",
		"--set",
		CLI_OUTPUT_OVERRIDE,
		"--set",
		CLI_SIZE_OVERRIDE,
		"--set",
		CLI_FORMAT_OVERRIDE,
	];
	let mut preview = overrides.to_vec();
	preview.push("--dry-run");
	let (stdout, stderr) = successful(run_project(&root, &preview));
	assert!(stdout.contains("mode=dry-run\n"));
	assert!(stdout.contains(&format!("logical_bytes={CLI_SIZE_BYTES}\n")));
	assert!(stdout.contains("physical_bytes=unknown\n"));
	assert!(stdout.contains("format=Udro\n"));
	let view = if cfg!(feature = "finder-write") {
		"IconView"
	} else {
		"NoStore"
	};
	assert!(stdout.contains(&format!("finder={view}\n")));
	assert!(stdout.contains("resolved_entries=1\n"));
	assert!(stdout.contains(&format!("source_payload_bytes={}\n", CONTENTS.len())));
	assert!(stdout.contains(&format!("{:?}", root.join(CLI_OUTPUT))));
	assert!(stderr.contains("Metadata loss at"));
	assert!(!stderr.contains("Encoding"));
	assert!(!stderr.contains("Published"));
	assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
	assert_eq!(
		fs::read_dir(root.join(CONFIG_DIRECTORY)).unwrap().count(),
		2
	);

	let (_, stderr) = successful(run_project(&root, &overrides));
	assert!(stderr.contains("Discovering source paths"));
	assert!(stderr.contains(&format!("Encoding {CLI_SIZE_BYTES} logical bytes")));
	assert!(stderr.contains("Checking sources before publication"));
	assert!(stderr.contains("Published"));
	let source = FileSource::new(fs::File::open(root.join(CLI_OUTPUT)).unwrap()).unwrap();
	let mut image = Image::open(source, Limits::default()).unwrap();
	assert_eq!(image.descriptor().logical_bytes, CLI_SIZE_BYTES);
	image.verify().unwrap();
	assert!(!root.join(CONFIG_DIRECTORY).join(TOML_OUTPUT).exists());
}

#[test]
fn reports_list_view_and_builds_the_root_and_nested_finder_assets() {
	check_default_view(LIST_CONFIG, LIST_OUTPUT, LIST_MANIFEST, "ListView", 2);
}

#[test]
fn reports_column_view_and_builds_the_root_and_nested_finder_assets() {
	check_default_view(
		COLUMN_CONFIG,
		COLUMN_OUTPUT,
		COLUMN_MANIFEST,
		"ColumnView",
		3,
	);
}

fn check_default_view(config: &str, image: &str, manifest: &str, view: &str, assets: usize) {
	let caller = tempfile::tempdir().unwrap();
	let root = caller.path().canonicalize().unwrap();
	fs::write(root.join(config), manifest).unwrap();
	let run = |extra: &[&str]| command(&root).arg(config).args(extra).output().unwrap();
	let preview = run(&["--dry-run"]);
	let destination = root.join(image);
	assert!(!destination.exists());
	assert_eq!(fs::read_dir(&root).unwrap().count(), 1);

	if !cfg!(feature = "finder-write") {
		for output in [preview, run(&[])] {
			assert!(!output.status.success());
			let stderr = String::from_utf8(output.stderr).unwrap();
			assert!(
				stderr.contains("feature not compiled: Finder layout"),
				"{stderr}"
			);
			assert!(!stderr.contains("Discovering source paths"));
		}
		assert!(!destination.exists());
		return;
	}

	let (summary, _) = successful(preview);
	assert!(summary.contains(&format!("finder={view}\n")));
	assert!(summary.contains(&format!("finder_assets={assets}\n")));
	assert!(summary.contains(&format!("logical_bytes={FINDER_FIXTURE_LOGICAL_BYTES}\n")));
	successful(run(&[]));
	let source = FileSource::new(fs::File::open(destination).unwrap()).unwrap();
	let mut image = Image::open(source, Limits::default()).unwrap();
	assert_eq!(
		image.descriptor().logical_bytes,
		FINDER_FIXTURE_LOGICAL_BYTES
	);
	image.verify().unwrap();
}

#[cfg(not(feature = "volume-icon"))]
#[test]
fn refuses_png_volume_icons_before_discovering_sources_when_disabled() {
	const PROJECT: &str = r#"
schema_version = 1

[image]
name = 'Volume icon feature test'
output = 'image.dmg'

[volume_icon.images]
'16' = 'absent.png'
"#;
	const PROJECT_NAME: &str = "volume-icon.toml";
	const IMAGE_NAME: &str = "image.dmg";
	let root = tempfile::tempdir().unwrap();
	fs::write(root.path().join(PROJECT_NAME), PROJECT).unwrap();
	let output = command(root.path())
		.args([PROJECT_NAME, "--dry-run"])
		.output()
		.unwrap();
	assert!(!output.status.success());
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(
		stderr.contains("feature not compiled: Finder volume icon"),
		"{stderr}"
	);
	assert!(!stderr.contains("Discovering source paths"));
	assert!(!root.path().join(IMAGE_NAME).exists());
}

#[cfg(not(feature = "volume-icon"))]
#[test]
fn refuses_custom_item_icons_before_discovering_sources_when_disabled() {
	const PROJECT: &str = r#"
schema_version = 1

[image]
name = 'Item icon feature test'
output = 'image.dmg'

[[contents]]
source = 'absent.txt'
custom_icon = 'absent.icns'
"#;
	const PROJECT_NAME: &str = "item-icon.toml";
	const IMAGE_NAME: &str = "image.dmg";
	let root = tempfile::tempdir().unwrap();
	fs::write(root.path().join(PROJECT_NAME), PROJECT).unwrap();
	let output = command(root.path())
		.args([PROJECT_NAME, "--dry-run"])
		.output()
		.unwrap();
	assert!(!output.status.success());
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(
		stderr.contains("feature not compiled: Finder custom item icon"),
		"{stderr}"
	);
	assert!(!stderr.contains("Discovering source paths"));
	assert!(!root.path().join(IMAGE_NAME).exists());
}

#[cfg(not(feature = "badge-icons"))]
#[test]
fn refuses_a_volume_badge_before_discovering_sources_when_disabled() {
	const PROJECT: &str = r#"
schema_version = 1

[image]
name = 'Volume badge feature test'
output = 'image.dmg'

[volume_icon]
badge = 'absent.png'
"#;
	const PROJECT_NAME: &str = "volume-badge.toml";
	const IMAGE_NAME: &str = "image.dmg";
	let root = tempfile::tempdir().unwrap();
	fs::write(root.path().join(PROJECT_NAME), PROJECT).unwrap();
	let output = command(root.path())
		.args([PROJECT_NAME, "--dry-run"])
		.output()
		.unwrap();
	assert!(!output.status.success());
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(
		stderr.contains("feature not compiled: Finder volume icon badge (badge-icons)"),
		"{stderr}"
	);
	assert!(!stderr.contains("Discovering source paths"));
	assert!(!root.path().join(IMAGE_NAME).exists());
}

#[test]
fn ulmo_requires_the_optional_codec_and_preserves_the_logical_image() {
	let caller = tempfile::tempdir().unwrap();
	let root = caller.path().canonicalize().unwrap();
	project(&root);
	let output = root.join(CONFIG_DIRECTORY).join(TOML_OUTPUT);
	let reproducible = [
		"--set",
		CLI_SIZE_OVERRIDE,
		"--set",
		"build.reproducible=true",
		"--set",
		"build.source_date_epoch=946684800",
	];
	let options = [&reproducible[..], &["--set", ULMO_FORMAT_OVERRIDE]].concat();
	let preview = run_project(&root, &[&options[..], &["--dry-run"]].concat());
	if !cfg!(feature = "lzma") {
		for refused in [preview, run_project(&root, &options)] {
			assert!(!refused.status.success());
			let stderr = String::from_utf8(refused.stderr).unwrap();
			assert!(
				stderr.contains("feature not compiled: LZMA2 encoding"),
				"{stderr}"
			);
			assert!(!stderr.contains("Discovering source paths"));
		}
		assert!(!output.exists());
		assert_eq!(
			fs::read_dir(root.join(CONFIG_DIRECTORY)).unwrap().count(),
			2
		);
		return;
	}

	let (summary, _) = successful(preview);
	assert!(summary.contains("format=Ulmo\n"));
	assert!(summary.contains(&format!("logical_bytes={CLI_SIZE_BYTES}\n")));
	assert!(!output.exists());
	successful(run_project(&root, &options));
	let source = FileSource::new(fs::File::open(output).unwrap()).unwrap();
	let mut ulmo = Image::open(source, Limits::default()).unwrap();
	assert!(
		ulmo.runs()
			.any(|run| run.encoding == RunEncoding::Lzma2 && run.encoding.can_decode())
	);
	ulmo.verify().unwrap();

	let baseline = [
		&reproducible[..],
		&["--set", CLI_FORMAT_OVERRIDE, "--set", CLI_OUTPUT_OVERRIDE],
	]
	.concat();
	successful(run_project(&root, &baseline));
	let source = FileSource::new(fs::File::open(root.join(CLI_OUTPUT)).unwrap()).unwrap();
	let mut udro = Image::open(source, Limits::default()).unwrap();
	let mut compressed = Vec::new();
	let mut raw = Vec::new();
	ulmo.write_raw_to(&mut compressed).unwrap();
	udro.write_raw_to(&mut raw).unwrap();
	assert_eq!(compressed.len() as u64, CLI_SIZE_BYTES);
	assert_eq!(compressed, raw);
}

#[test]
fn preserves_existing_output_until_explicit_overwrite() {
	let caller = tempfile::tempdir().unwrap();
	let root = caller.path().canonicalize().unwrap();
	project(&root);
	let output = root.join(CONFIG_DIRECTORY).join(TOML_OUTPUT);
	fs::write(&output, PREVIOUS).unwrap();
	let refused = run_project(&root, &[]);
	assert!(!refused.status.success());
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS);
	assert_eq!(
		fs::read_dir(root.join(CONFIG_DIRECTORY)).unwrap().count(),
		3
	);

	let conflicting = run_project(&root, &["--overwrite", "--set", "build.overwrite=false"]);
	assert!(!conflicting.status.success());
	let diagnostics = String::from_utf8(conflicting.stderr).unwrap();
	assert!(diagnostics.contains("choose either --overwrite or --set build.overwrite"));
	assert!(!diagnostics.contains("Discovering source paths"));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS);

	let (report, _) = successful(run_project(&root, &["--overwrite", "--dry-run"]));
	assert!(report.contains("publication=Path { overwrite: true }"));
	assert_eq!(fs::read(&output).unwrap(), PREVIOUS);
	assert_eq!(
		fs::read_dir(root.join(CONFIG_DIRECTORY)).unwrap().count(),
		3
	);

	successful(run_project(&root, &["--overwrite"]));
	let source = FileSource::new(fs::File::open(&output).unwrap()).unwrap();
	Image::open(source, Limits::default())
		.unwrap()
		.verify()
		.unwrap();
	assert_eq!(
		fs::read_dir(root.join(CONFIG_DIRECTORY)).unwrap().count(),
		3
	);
}

#[test]
fn rejects_bad_typed_values_without_echoing_the_value() {
	let caller = tempfile::tempdir().unwrap();
	let root = caller.path().canonicalize().unwrap();
	project(&root);
	const PRIVATE_VALUE: &str = "an-opaque-argument-value";
	let assignment = format!("image.size={PRIVATE_VALUE}");
	let output = run_project(&root, &["--dry-run", "--set", &assignment]);
	assert!(!output.status.success());
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(stderr.contains("invalid configuration override value for image.size"));
	assert!(!stderr.contains(PRIVATE_VALUE));
	assert_eq!(
		fs::read_dir(root.join(CONFIG_DIRECTORY)).unwrap().count(),
		2
	);
}
