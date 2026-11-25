use std::{
	fs,
	num::NonZeroUsize,
	os::unix::fs::{PermissionsExt, symlink},
};

use dmgez_builder::preflight::{Availability, OutputIntent, PreflightOptions, ReleaseIntent};
use dmgez_config::Project;

use super::*;

const APP: &str = "Witness.app";
const VIRTUAL_APP: &str = "/Folder/Witness.app";
const CONTENTS: &str = "Contents";
const EXECUTABLE: &str = "Contents/MacOS/Witness";
const PLIST: &str = "Contents/Info.plist";
const ALIAS: &str = "Contents/Entrypoint";
const LINK: &str = "MacOS/Witness";
const RESOURCE_FORK: &str = "..namedfork/rsrc";
const RESOURCE: &[u8] = b"sealed resource metadata";
const SOURCE_PROGRAM: &[u8] = b"#!/bin/false\n";
const OTHER_PROGRAM: &[u8] = b"#!/bin/true!\n";
const EXECUTABLE_MODE: u32 = 0o751;
const SETTINGS: &str = r#"
schema_version = 1
[image]
name = "Release Application Fixture"
output = "release.dmg"
format = "udro"
size = "2MiB"
[metadata]
policy = "report"
preserve_timestamps = false
preserve_extended_attributes = false
preserve_finder_info = false
[[contents]]
source = "Witness.app"
destination = "/Folder/Witness.app"
[[contents]]
type = "directory"
destination = "/Folder/Witness.app-other"
"#;

fn fixture(root: &Path, executable: &[u8]) {
	let app = root.join(APP);
	let binary = app.join(EXECUTABLE);
	fs::create_dir_all(binary.parent().unwrap()).unwrap();
	fs::write(&binary, executable).unwrap();
	fs::set_permissions(&binary, fs::Permissions::from_mode(EXECUTABLE_MODE)).unwrap();
	fs::write(binary.join(RESOURCE_FORK), RESOURCE).unwrap();
	fs::write(
		app.join(PLIST),
		br#"<plist><dict><key>CFBundleExecutable</key><string>Witness</string></dict></plist>"#,
	)
	.unwrap();
	symlink(LINK, app.join(ALIAS)).unwrap();
}

fn prepared<'a>(root: &Path, project: &'a Project) -> dmgez_hfsplus::PreparedHfsBuild<'a> {
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.verify = Availability::Available;
	capabilities.release = Availability::Available;
	dmgez_hfsplus::prepare(
		project,
		root,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Release,
			host_threads: NonZeroUsize::MIN,
		},
		Default::default(),
	)
	.unwrap()
}

#[test]
fn reconstructs_exact_bundle_and_runs_verifier_on_private_copy_of_unsigned_image() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path().canonicalize().unwrap();
	fixture(&root, SOURCE_PROGRAM);
	let project = Project::parse(SETTINGS).unwrap();
	let build = prepared(&root, &project);
	let applications = source::prepare(build.prepared(), |path| {
		assert_eq!(path, root.join(APP));
		Ok(())
	})
	.unwrap();
	assert_eq!(applications, [VIRTUAL_APP.parse().unwrap()]);
	// Verify scratch cleanup also handles an owner-readable but read-only tree.
	let stage = build
		.seal()
		.unwrap()
		.stage_release(&release_artifact::verify_stage, |_| {})
		.unwrap();
	let identity = release_artifact::observe(stage.path()).unwrap();
	let mut private = None;
	let count = verify_staged(
		stage.path(),
		&identity,
		build.prepared().manifest(),
		&applications,
		|path| {
			assert_ne!(path, root.join(APP));
			assert_eq!(fs::read(path.join(EXECUTABLE))?, SOURCE_PROGRAM);
			assert_eq!(
				fs::read(path.join(EXECUTABLE).join(RESOURCE_FORK))?,
				RESOURCE
			);
			assert_eq!(fs::read_link(path.join(ALIAS))?, Path::new(LINK));
			assert_eq!(
				fs::metadata(path.join(EXECUTABLE))?.permissions().mode() & u32::from(PERMISSIONS),
				EXECUTABLE_MODE
			);
			fs::set_permissions(path.join(CONTENTS), fs::Permissions::from_mode(0o555))?;
			private = Some(path.to_owned());
			Ok(())
		},
	)
	.unwrap();
	assert_eq!(count, 1);
	assert!(!private.unwrap().exists());
	assert_eq!(release_artifact::observe(stage.path()).unwrap(), identity);
	assert!(!build.prepared().output_path().unwrap().exists());
}

#[test]
fn refuses_unexpected_applications_and_same_length_wrong_payload_before_policy_check() {
	let first = tempfile::tempdir().unwrap();
	let second = tempfile::tempdir().unwrap();
	let first_root = first.path().canonicalize().unwrap();
	let second_root = second.path().canonicalize().unwrap();
	assert_eq!(SOURCE_PROGRAM.len(), OTHER_PROGRAM.len());
	fixture(&first_root, SOURCE_PROGRAM);
	fixture(&second_root, OTHER_PROGRAM);
	let project = Project::parse(SETTINGS).unwrap();
	let with_original = prepared(&first_root, &project);
	let with_other = prepared(&second_root, &project);
	let stage = with_original
		.seal()
		.unwrap()
		.stage_release(&release_artifact::verify_stage, |_| {})
		.unwrap();
	let identity = release_artifact::observe(stage.path()).unwrap();
	let paths = [VIRTUAL_APP.parse().unwrap()];
	for (manifest, applications, message) in [
		(
			with_original.prepared().manifest(),
			&[][..],
			"unexpected staged application",
		),
		(with_other.prepared().manifest(), &paths[..], EXECUTABLE),
	] {
		let failed = verify_staged(stage.path(), &identity, manifest, applications, |_| {
			panic!("policy verification must not run on a different assembled application")
		})
		.unwrap_err();
		assert!(failed.to_string().contains(message), "{failed}");
	}
}
