use std::{
	path::Path,
	process::{Command, Output},
};

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const PROFILE: &str = "installer";
const PROFILE_CAPABILITY: &str = "installer-lint";
const PROJECT_FILE: &str = "project.toml";
const ERROR: i32 = 2;

fn lint(project: &Path) -> Command {
	let mut command = Command::new(BINARY);
	command
		.arg("lint")
		.arg(project)
		.args(["--profile", PROFILE]);
	command
}

fn assert_refused(result: &Output, expected: &str) {
	let diagnostic = String::from_utf8_lossy(&result.stderr);
	assert_eq!(result.status.code(), Some(ERROR), "{diagnostic}");
	assert!(result.stdout.is_empty(), "{result:?}");
	assert!(diagnostic.contains(expected), "{diagnostic}");
}

#[cfg(not(all(feature = "installer-lint", any(unix, windows))))]
#[test]
fn unavailable_installer_profile_refuses_before_opening_the_project() {
	let output = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let capabilities = String::from_utf8_lossy(&output.stdout);
	let expected = format!("{PROFILE_CAPABILITY} = not_compiled");
	assert!(
		capabilities.lines().any(|line| line == expected),
		"{capabilities}"
	);
	let directory = tempfile::tempdir().unwrap();
	let missing = directory.path().join(PROJECT_FILE);
	let result = lint(&missing).arg("--json").output().unwrap();
	if cfg!(feature = "installer-lint") {
		assert_refused(&result, "installer lint requires host source discovery");
	} else {
		assert_refused(
			&result,
			&format!("feature not compiled: {PROFILE_CAPABILITY}"),
		);
	}
	assert!(!missing.exists());
}

#[cfg(all(feature = "installer-lint", any(unix, windows)))]
#[test]
fn installer_profile_rejects_folder_selection_before_opening_the_project() {
	let output = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let capabilities = String::from_utf8_lossy(&output.stdout);
	let expected = format!("{PROFILE_CAPABILITY} = compiled");
	assert!(
		capabilities.lines().any(|line| line == expected),
		"{capabilities}"
	);
	let directory = tempfile::tempdir().unwrap();
	let missing = directory.path().join(PROJECT_FILE);
	let result = lint(&missing)
		.args(["--folder", "/", "--json"])
		.output()
		.unwrap();
	assert_refused(
		&result,
		"--folder is only supported by the layout lint profile",
	);
}

#[cfg(all(feature = "installer-lint", unix))]
mod unix {
	use std::{
		fs,
		os::unix::fs::{MetadataExt, PermissionsExt, symlink},
		path::PathBuf,
	};

	use serde_json::Value;

	use super::{PROFILE, PROJECT_FILE, assert_refused, lint};

	const SUCCESS: i32 = 0;
	const WARNINGS: i32 = 1;
	const SCHEMA_VERSION: u32 = 1;
	const APP_DIRECTORY: &str = "Example.app";
	const APP_DESTINATION: &str = "/Example.app";
	const CONTENTS_DIRECTORY: &str = "Contents";
	const INFO_PLIST: &str = "Info.plist";
	const EXECUTABLE_DIRECTORY: &str = "MacOS";
	const EXECUTABLE_NAME: &str = "Example";
	const EXECUTABLE_KEY: &str = "CFBundleExecutable";
	const EXECUTABLE_BYTES: &[u8] = b"synthetic installer executable bytes\n";
	const IMAGE_FILE: &str = "existing.dmg";
	const EXECUTABLE_MODE: u32 = 0o755;
	const NONEXECUTABLE_MODE: u32 = 0o644;
	const MODE_MASK: u32 = 0o777;
	const APPLICATIONS_PATH: &str = "/Applications";
	const MISSING_APPLICATIONS: &str = "missing_applications_link";
	const INVALID_APPLICATIONS: &str = "invalid_applications_link";
	const EXTERNAL_SYMLINK: &str = "external_symlink";
	const MISSING_BUNDLE_INFO: &str = "bundle_info_missing";
	const INVALID_BUNDLE_INFO: &str = "bundle_info_invalid";
	const NOT_REGULAR_FILE: &str = "not_regular_file";
	const MISSING_EXECUTABLE: &str = "bundle_executable_missing";
	const NONEXECUTABLE: &str = "bundle_executable_not_executable";
	const MODE_NOT_PRESERVED: &str = "bundle_executable_permissions_not_preserved";
	const MISSING_BACKGROUND: &str = "background_missing";
	const SUPPRESS_OPTION: &str = "--suppress";

	struct Fixture {
		_directory: tempfile::TempDir,
		project: PathBuf,
		info: PathBuf,
		executable: PathBuf,
		output: PathBuf,
	}

	impl Fixture {
		fn new() -> Self {
			let directory = tempfile::tempdir().unwrap();
			let root = directory.path();
			let contents = root.join(APP_DIRECTORY).join(CONTENTS_DIRECTORY);
			let executables = contents.join(EXECUTABLE_DIRECTORY);
			fs::create_dir_all(&executables).unwrap();
			Self {
				project: root.join(PROJECT_FILE),
				info: contents.join(INFO_PLIST),
				executable: executables.join(EXECUTABLE_NAME),
				output: root.join(IMAGE_FILE),
				_directory: directory,
			}
		}

		fn write_project(&self, declarations: &str) -> String {
			let project = format!(
				"schema_version = 1\n[image]\nname = 'Example'\noutput = '{IMAGE_FILE}'\n\
				[[contents]]\nsource = '{APP_DIRECTORY}'\ndestination = '{APP_DESTINATION}'\n\
				{declarations}"
			);
			fs::write(&self.project, &project).unwrap();
			project
		}

		fn write_bundle_info(&self, executable: &str) -> String {
			let plist = format!(
				r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>{EXECUTABLE_KEY}</key><string>{executable}</string></dict></plist>"#
			);
			fs::write(&self.info, &plist).unwrap();
			plist
		}

		fn write_executable(&self, mode: u32) {
			fs::write(&self.executable, EXECUTABLE_BYTES).unwrap();
			fs::set_permissions(&self.executable, fs::Permissions::from_mode(mode)).unwrap();
		}

		fn report(&self, args: &[&str], exit: i32) -> Value {
			let result = lint(&self.project)
				.arg("--json")
				.args(args)
				.output()
				.unwrap();
			assert_eq!(result.status.code(), Some(exit), "{result:?}");
			assert!(result.stderr.is_empty(), "{result:?}");
			let report: Value = serde_json::from_slice(&result.stdout).unwrap();
			assert_eq!(report["profile"], PROFILE, "{report}");
			assert_eq!(report["schema_version"], SCHEMA_VERSION, "{report}");
			report
		}
	}

	fn link(destination: &str, target: &str) -> String {
		format!(
			"[[contents]]\ntype = 'symlink'\ndestination = '{destination}'\ntarget = '{target}'\n"
		)
	}

	fn applications_link() -> String {
		link(APPLICATIONS_PATH, APPLICATIONS_PATH)
	}

	fn diagnostics(report: &Value) -> &[Value] {
		report["diagnostics"].as_array().unwrap()
	}

	fn finding<'input>(report: &'input Value, rule: &str) -> &'input Value {
		let finding = diagnostics(report)
			.iter()
			.find(|finding| finding["rule"] == rule)
			.unwrap_or_else(|| panic!("missing {rule} in {report}"));
		assert_eq!(finding["severity"], "warning", "{finding}");
		assert_eq!(finding["evidence"]["kind"], rule, "{finding}");
		finding
	}

	#[test]
	fn valid_installer_accepts_the_app_and_leaves_an_existing_output_alias_untouched() {
		let fixture = Fixture::new();
		let project = fixture.write_project(&applications_link());
		let plist = fixture.write_bundle_info(EXECUTABLE_NAME);
		fixture.write_executable(EXECUTABLE_MODE);
		fs::hard_link(&fixture.executable, &fixture.output).unwrap();
		let original_inode = fs::metadata(&fixture.executable).unwrap().ino();

		let report = fixture.report(&[], SUCCESS);
		assert!(diagnostics(&report).is_empty(), "{report}");
		assert_eq!(fs::read_to_string(&fixture.project).unwrap(), project);
		assert_eq!(fs::read_to_string(&fixture.info).unwrap(), plist);
		assert_eq!(fs::read(&fixture.executable).unwrap(), EXECUTABLE_BYTES);
		assert_eq!(fs::read(&fixture.output).unwrap(), EXECUTABLE_BYTES);
		assert_eq!(
			fs::metadata(&fixture.executable).unwrap().ino(),
			original_inode
		);
		assert_eq!(fs::metadata(&fixture.output).unwrap().ino(), original_inode);
		assert_eq!(
			fs::metadata(&fixture.executable)
				.unwrap()
				.permissions()
				.mode() & MODE_MASK,
			EXECUTABLE_MODE
		);
	}

	#[test]
	fn diagnoses_wrong_shortcut_and_volume_escape_but_accepts_an_internal_link() {
		const ESCAPING: &str = "/Links/Outside";
		const INTERNAL: &str = "/Links/Local";
		let fixture = Fixture::new();
		fixture.write_bundle_info(EXECUTABLE_NAME);
		fixture.write_executable(EXECUTABLE_MODE);
		let declarations = [
			link(APPLICATIONS_PATH, APP_DESTINATION.trim_start_matches('/')),
			link(ESCAPING, "../../outside"),
			link(INTERNAL, &format!("../{APP_DIRECTORY}")),
		]
		.concat();
		fixture.write_project(&declarations);

		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 2, "{report}");
		assert_eq!(
			finding(&report, INVALID_APPLICATIONS)["evidence"]["destination"],
			APPLICATIONS_PATH
		);
		assert_eq!(
			finding(&report, EXTERNAL_SYMLINK)["evidence"]["destination"],
			ESCAPING
		);
		assert!(!fixture.output.exists());
	}

	#[test]
	fn reports_missing_shortcut_background_bundle_metadata_and_executable() {
		let fixture = Fixture::new();
		fixture
			.write_project("[finder]\nbackground = { kind = 'image', source = 'missing.png' }\n");
		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 3, "{report}");
		finding(&report, MISSING_APPLICATIONS);
		finding(&report, MISSING_BACKGROUND);
		finding(&report, MISSING_BUNDLE_INFO);

		fixture.write_project(&applications_link());
		fixture.write_bundle_info("../outside");
		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 1, "{report}");
		finding(&report, INVALID_BUNDLE_INFO);

		fixture.write_bundle_info(EXECUTABLE_NAME);
		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 1, "{report}");
		finding(&report, MISSING_EXECUTABLE);
		assert!(!fixture.output.exists());
	}

	#[test]
	fn a_symlink_does_not_supply_bundle_metadata() {
		const OTHER_PLIST: &str = "Other.plist";
		let fixture = Fixture::new();
		fixture.write_project(&applications_link());
		let original = fixture.write_bundle_info(EXECUTABLE_NAME);
		fixture.write_executable(EXECUTABLE_MODE);
		let target = fixture.info.with_file_name(OTHER_PLIST);
		fs::rename(&fixture.info, &target).unwrap();
		symlink(OTHER_PLIST, &fixture.info).unwrap();

		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 1, "{report}");
		assert_eq!(
			finding(&report, INVALID_BUNDLE_INFO)["evidence"]["reason"],
			NOT_REGULAR_FILE
		);
		assert_eq!(
			fs::read_link(&fixture.info).unwrap(),
			PathBuf::from(OTHER_PLIST)
		);
		assert_eq!(fs::read_to_string(&target).unwrap(), original);
	}

	#[test]
	fn warns_when_project_policy_would_discard_executable_permissions() {
		let fixture = Fixture::new();
		fixture.write_project(&format!(
			"{}[metadata]\npreserve_permissions = false\n",
			applications_link()
		));
		fixture.write_bundle_info(EXECUTABLE_NAME);
		fixture.write_executable(EXECUTABLE_MODE);

		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 1, "{report}");
		assert_eq!(
			finding(&report, MODE_NOT_PRESERVED)["evidence"]["destination"],
			format!(
				"{APP_DESTINATION}/{CONTENTS_DIRECTORY}/{EXECUTABLE_DIRECTORY}/{EXECUTABLE_NAME}"
			)
		);
		assert_eq!(
			fs::metadata(&fixture.executable)
				.unwrap()
				.permissions()
				.mode() & MODE_MASK,
			EXECUTABLE_MODE
		);
	}

	#[test]
	fn non_executable_mode_is_reported_and_a_reasoned_suppression_keeps_the_finding() {
		const REASON: &str = "reviewed=test-only";
		let fixture = Fixture::new();
		let project = fixture.write_project(&applications_link());
		let plist = fixture.write_bundle_info(EXECUTABLE_NAME);
		fixture.write_executable(NONEXECUTABLE_MODE);
		let report = fixture.report(&[], WARNINGS);
		assert_eq!(diagnostics(&report).len(), 1, "{report}");
		assert!(finding(&report, NONEXECUTABLE)["suppression_reason"].is_null());

		let suppression = format!("{NONEXECUTABLE}={REASON}");
		let report = fixture.report(&[SUPPRESS_OPTION, &suppression], SUCCESS);
		assert_eq!(diagnostics(&report).len(), 1, "{report}");
		assert_eq!(
			finding(&report, NONEXECUTABLE)["suppression_reason"],
			REASON
		);
		let text = lint(&fixture.project)
			.args([SUPPRESS_OPTION, &suppression])
			.output()
			.unwrap();
		assert!(text.status.success(), "{text:?}");
		let text = String::from_utf8(text.stdout).unwrap();
		assert!(text.contains(&format!("profile = {PROFILE}")), "{text}");
		assert!(text.contains(&format!("warning {NONEXECUTABLE}")), "{text}");
		assert!(text.contains(&format!("(suppressed {REASON:?})")), "{text}");

		let blank_reason = format!("{NONEXECUTABLE}= ");
		let refused = lint(&fixture.project)
			.args([SUPPRESS_OPTION, &blank_reason, "--json"])
			.output()
			.unwrap();
		assert_refused(&refused, "requires a reason");
		assert_eq!(fs::read_to_string(&fixture.project).unwrap(), project);
		assert_eq!(fs::read_to_string(&fixture.info).unwrap(), plist);
		assert_eq!(fs::read(&fixture.executable).unwrap(), EXECUTABLE_BYTES);
		assert_eq!(
			fs::metadata(&fixture.executable)
				.unwrap()
				.permissions()
				.mode() & MODE_MASK,
			NONEXECUTABLE_MODE
		);
		assert!(!fixture.output.exists());
	}
}
