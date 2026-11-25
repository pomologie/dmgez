use std::process::Command;

#[cfg(all(
	feature = "project-build",
	any(target_os = "linux", target_os = "macos")
))]
#[path = "project_build/compression_cache.rs"]
mod compression_cache;
#[cfg(all(
	feature = "project-build",
	feature = "finder-write",
	feature = "hfsplus-read"
))]
#[path = "project_build/finder_flags.rs"]
mod finder_flags;
#[cfg(feature = "project-release")]
#[path = "project_build/release.rs"]
mod release;
#[cfg(feature = "project-build")]
#[path = "project_build/udrw.rs"]
mod udrw;

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const MANIFEST_NAME: &str = "installer.toml";

#[cfg(not(feature = "project-build"))]
#[test]
fn reports_an_unavailable_project_build_before_reading_the_manifest() {
	let directory = tempfile::tempdir().unwrap();
	let result = Command::new(BINARY)
		.current_dir(directory.path())
		.args(["build", MANIFEST_NAME, "--dry-run"])
		.output()
		.unwrap();
	assert!(!result.status.success());
	assert!(result.stdout.is_empty());
	assert!(String::from_utf8_lossy(&result.stderr).contains("requires an HFS+ composed CLI"));
	assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[cfg(feature = "project-build")]
#[test]
fn previews_and_publishes_a_project_with_caller_relative_overrides() {
	use std::fs;

	const PROJECT_DIRECTORY: &str = "configuration";
	const MANIFEST_OUTPUT: &str = "manifest.dmg";
	const CALLER_OUTPUT: &str = "caller.dmg";
	const CALLER_OUTPUT_OVERRIDE: &str = "image.output=caller.dmg";
	const EXACT_SIZE_OVERRIDE: &str = "image.size=1MiB";
	const EXACT_SIZE_BYTES: u64 = 1_048_576;
	const TITLE_DEFINITION: &str = "TITLE=Composed Installer";
	const TRAILER_BYTES: usize = 512;
	const TRAILER_MAGIC: &[u8] = b"koly";
	const SETTINGS: &str = r#"
schema_version = 1

[image]
name = "${TITLE}"
output = "manifest.dmg"
format = "udro"

[[contents]]
type = "directory"
destination = "/Data"
"#;

	let directory = tempfile::tempdir().unwrap();
	let caller = directory.path().canonicalize().unwrap();
	let configuration = caller.join(PROJECT_DIRECTORY);
	fs::create_dir(&configuration).unwrap();
	let manifest = configuration.join(MANIFEST_NAME);
	fs::write(&manifest, SETTINGS).unwrap();
	let output_path = caller.join(CALLER_OUTPUT);
	let run = |options: &[&str]| {
		Command::new(BINARY)
			.current_dir(&caller)
			.arg("build")
			.arg(manifest.strip_prefix(&caller).unwrap())
			.args([
				"--set",
				CALLER_OUTPUT_OVERRIDE,
				"--set",
				EXACT_SIZE_OVERRIDE,
				"--define",
				TITLE_DEFINITION,
			])
			.args(options)
			.output()
			.unwrap()
	};

	let preview = run(&["--dry-run"]);
	assert!(
		preview.status.success(),
		"{}",
		String::from_utf8_lossy(&preview.stderr)
	);
	let plan = String::from_utf8(preview.stdout).unwrap();
	assert!(plan.contains("mode=dry-run\n"));
	assert!(plan.contains("filesystem=HfsplusUnicode\n"));
	assert!(plan.contains(&format!("logical_bytes={EXACT_SIZE_BYTES}\n")));
	assert!(plan.contains("physical_bytes=unknown\n"));
	assert!(!output_path.exists());

	let build = run(&[]);
	assert!(
		build.status.success(),
		"{}",
		String::from_utf8_lossy(&build.stderr)
	);
	let image = fs::read(&output_path).unwrap();
	assert!(image.len() >= TRAILER_BYTES);
	assert!(image[image.len() - TRAILER_BYTES..].starts_with(TRAILER_MAGIC));
	assert!(!configuration.join(MANIFEST_OUTPUT).exists());
	assert_eq!(fs::read_to_string(&manifest).unwrap(), SETTINGS);
	assert!(String::from_utf8_lossy(&build.stderr).contains("Published"));
	assert!(
		String::from_utf8_lossy(&build.stdout)
			.contains(&format!("{EXACT_SIZE_BYTES} logical bytes"))
	);
}

#[cfg(feature = "project-build")]
#[test]
fn unsigned_build_reports_configured_release_stages_as_deferred() {
	use std::fs;

	const OUTPUT_NAME: &str = "unsigned.dmg";
	const SETTINGS: &str = r#"
schema_version = 1

[image]
name = "Unsigned Fixture"
output = "unsigned.dmg"
format = "udro"

[[contents]]
type = "directory"
destination = "/Data"

[signing]
enabled = true
dmg = true

[notarization]
enabled = true

[release]
require_native_assessment = true
"#;
	let directory = tempfile::tempdir().unwrap();
	fs::write(directory.path().join(MANIFEST_NAME), SETTINGS).unwrap();
	let build = Command::new(BINARY)
		.current_dir(directory.path())
		.args(["build", MANIFEST_NAME])
		.output()
		.unwrap();
	let stderr = String::from_utf8(build.stderr).unwrap();
	assert!(build.status.success(), "{stderr}");
	assert!(
		stderr.contains("Configured release stages were deferred. Published an unsigned image.")
	);
	assert!(directory.path().join(OUTPUT_NAME).is_file());
}

#[cfg(all(feature = "project-build", target_os = "macos"))]
#[test]
fn prepares_a_native_valid_hfsplus_release_image_in_a_temporary_stage() {
	use std::{
		io::{self, Read, Seek, SeekFrom},
		num::NonZeroUsize,
	};

	use dmgez_builder::preflight::{Availability, OutputIntent, PreflightOptions, ReleaseIntent};
	use dmgez_config::Project;
	use dmgez_udif::{TRAILER_BYTES, check_trailer_header};

	const DESTINATION: &str = "installer.dmg";
	const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Release Stage"
output = "installer.dmg"
format = "udro"
[[contents]]
type = "directory"
destination = "/Data"
"#;

	let directory = tempfile::tempdir().unwrap();
	let parent = directory.path().canonicalize().unwrap();
	let project = Project::parse(SETTINGS).unwrap();
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.release = Availability::Available;
	let prepared = dmgez_hfsplus::prepare(
		&project,
		&parent,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Release,
			host_threads: NonZeroUsize::MIN,
		},
		Default::default(),
	)
	.unwrap();
	let image = prepared.seal().unwrap();
	let stage = image
		.stage_release(
			&|mut reader, _| {
				let mut trailer = [0; TRAILER_BYTES];
				reader.seek(SeekFrom::End(-(TRAILER_BYTES as i64)))?;
				reader.read_exact(&mut trailer)?;
				check_trailer_header(&trailer).map_err(io::Error::other)
			},
			|_| {},
		)
		.unwrap();
	let result = Command::new("/usr/bin/hdiutil")
		.args(["verify", "-quiet"])
		.arg(stage.path())
		.output()
		.unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	assert!(!parent.join(DESTINATION).exists());
	drop(stage);
	assert_eq!(std::fs::read_dir(parent).unwrap().count(), 0);
}
