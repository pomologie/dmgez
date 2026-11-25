use std::{fmt::Write as _, fs, io::ErrorKind, path::Path, process::Output};

use sha2::{Digest, Sha256};

use super::{BINARY, MANIFEST_NAME};

const OUTPUT_NAME: &str = "release.dmg";
const PROJECT: &str = r#"
schema_version = 1
[image]
name = "Release Fixture"
output = "release.dmg"
format = "udro"
[[contents]]
type = "directory"
destination = "/Data"
[release]
verify = true
"#;

fn run(directory: &Path, additional: &[&str]) -> Output {
	std::process::Command::new(BINARY)
		.current_dir(directory)
		.args(["build", MANIFEST_NAME, "--release", "--json"])
		.args(additional)
		.output()
		.unwrap()
}

#[test]
fn publishes_a_verified_unsigned_release_and_binds_the_report_to_its_bytes() {
	let directory = tempfile::tempdir().unwrap();
	let manifest = directory.path().join(MANIFEST_NAME);
	let image_path = directory.path().join(OUTPUT_NAME);
	fs::write(&manifest, PROJECT).unwrap();
	let output = run(directory.path(), &[]);
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
	let image = fs::read(&image_path).unwrap();
	let mut digest = String::new();
	for byte in Sha256::digest(&image) {
		write!(digest, "{byte:02x}").unwrap();
	}
	assert_eq!(report["artifact"]["sha256"], digest);
	assert_eq!(report["artifact"]["bytes"], image.len() as u64);
	let step = |name| {
		report["steps"]
			.as_array()
			.unwrap()
			.iter()
			.find(|step| step["stage"] == name)
			.unwrap()
	};
	assert_eq!(step("verify_image")["status"], "passed");
	assert_eq!(step("sign_image")["status"], "skipped");
	assert_eq!(step("publish")["status"], "passed");
	assert_eq!(fs::read_to_string(&manifest).unwrap(), PROJECT);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);

	let retry = run(directory.path(), &[]);
	assert!(!retry.status.success());
	assert_eq!(fs::read(image_path).unwrap(), image);
}

#[test]
fn rejects_a_requested_unavailable_backend_and_an_implicit_overwrite() {
	let directory = tempfile::tempdir().unwrap();
	let manifest = directory.path().join(MANIFEST_NAME);
	let image = directory.path().join(OUTPUT_NAME);
	let signing = "[signing]\nenabled = true\napps = 'preserve'\ndmg = true\nbackend = 'rust'\n";
	fs::write(&manifest, format!("{PROJECT}\n{signing}")).unwrap();
	assert!(!run(directory.path(), &[]).status.success());
	assert_eq!(
		fs::metadata(&image).unwrap_err().kind(),
		ErrorKind::NotFound
	);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

	fs::write(&manifest, PROJECT).unwrap();
	assert!(
		!run(directory.path(), &["--set", "build.overwrite=true"])
			.status
			.success()
	);
	assert_eq!(fs::metadata(image).unwrap_err().kind(), ErrorKind::NotFound);
	assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}
