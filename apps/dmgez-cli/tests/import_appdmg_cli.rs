use std::{fs, path::Path, process::Command};

const SETTINGS_NAME: &str = "settings.json";
const PROJECT_NAME: &str = "dmg.toml";
const IMAGE_NAME: &str = "installer.dmg";

fn import(settings: &Path, output: &Path) -> Command {
	let mut command = Command::new(env!("CARGO_BIN_EXE_dmgez"));
	command
		.arg("import-appdmg")
		.arg(settings)
		.arg("--output")
		.arg(output)
		.args(["--image-output", IMAGE_NAME]);
	command
}

#[cfg(not(feature = "appdmg-import"))]
#[test]
fn reports_an_uncompiled_import_without_writing_a_manifest() {
	let dir = tempfile::tempdir().unwrap();
	let output = dir.path().join(PROJECT_NAME);
	let command = import(&dir.path().join(SETTINGS_NAME), &output)
		.output()
		.unwrap();
	assert!(!command.status.success());
	assert!(
		String::from_utf8_lossy(&command.stderr).contains("feature not compiled: appdmg-import")
	);
	assert!(!fs::exists(output).unwrap());
}

#[cfg(feature = "appdmg-import")]
#[test]
fn writes_valid_toml_beside_a_different_directory_and_preserves_existing_output() {
	use dmgez_config::{Payload, Project};

	const INPUT: &str = r#"{
		"title": "Example Installer",
		"contents": [
			{"x": 100, "y": 100, "type": "file", "path": "Example.app"},
			{"x": 250, "y": 100, "type": "link", "path": "/Applications"}
		]
	}"#;
	let dir = tempfile::tempdir().unwrap();
	let source_directory = dir.path().join("legacy");
	fs::create_dir(&source_directory).unwrap();
	let settings = source_directory.join(SETTINGS_NAME);
	fs::write(&settings, INPUT).unwrap();
	let output = dir.path().join(PROJECT_NAME);
	let result = import(&settings, &output).output().unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
	let manifest = fs::read_to_string(&output).unwrap();
	let project = Project::parse(&manifest).unwrap();
	assert_eq!(project.image.name.as_str(), "Example Installer");
	assert_eq!(project.image.output.as_path(), Path::new(IMAGE_NAME));
	let app = project
		.contents
		.iter()
		.find(|item| item.destination.as_str() == "/Example.app")
		.unwrap();
	let Payload::Copy(source) = &app.payload else {
		panic!("expected a copy declaration");
	};
	assert_eq!(
		source.source.as_path(),
		source_directory.join("Example.app")
	);
	assert_eq!(
		app.position.unwrap().map(|coordinate| coordinate.get()),
		[100.0, 100.0]
	);
	let applications = project
		.contents
		.iter()
		.find(|item| item.destination.as_str() == "/Applications")
		.unwrap();
	assert!(
		matches!(&applications.payload, Payload::Symlink { target } if target.as_str() == "/Applications")
	);
	assert_eq!(fs::read_to_string(&settings).unwrap(), INPUT);
	assert!(!dir.path().join(IMAGE_NAME).exists());

	let result = import(&settings, &output).output().unwrap();
	assert!(!result.status.success());
	assert_eq!(fs::read_to_string(&output).unwrap(), manifest);
}

#[cfg(feature = "appdmg-import")]
#[test]
fn rejects_unknown_json_without_publishing_toml() {
	let dir = tempfile::tempdir().unwrap();
	let settings = dir.path().join(SETTINGS_NAME);
	fs::write(
		&settings,
		r#"{"title":"Example","contents":[],"shell":"touch unexpected"}"#,
	)
	.unwrap();
	let output = dir.path().join(PROJECT_NAME);
	let result = import(&settings, &output).output().unwrap();
	assert!(!result.status.success());
	assert!(!result.stderr.is_empty());
	assert!(result.stdout.is_empty());
	assert!(!output.exists());
}
