use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_dmgez");
const CAPABILITY: &str = "signing-native";
const RUST_CAPABILITY: &str = "signing-rust";
const INSPECT: &str = "inspect";
const VERIFY: &str = "verify";
const ERROR: i32 = 2;
const MISSING_APP: &str = "missing.app";

fn sign(operation: &str) -> Command {
	let mut command = Command::new(BINARY);
	command.args(["sign", operation]);
	command
}

#[test]
fn reports_signing_capability() {
	let output = Command::new(BINARY).arg("capabilities").output().unwrap();
	assert!(output.status.success(), "{output:?}");
	let capabilities = String::from_utf8_lossy(&output.stdout);
	for (capability, compiled, supported) in [
		(
			CAPABILITY,
			cfg!(feature = "signing-native"),
			cfg!(target_os = "macos"),
		),
		(
			RUST_CAPABILITY,
			cfg!(feature = "signing-rust"),
			cfg!(any(target_os = "linux", target_os = "macos", windows)),
		),
	] {
		let state = match (compiled, supported) {
			(true, true) => "compiled",
			(true, false) => "unavailable",
			(false, _) => "not_compiled",
		};
		let expected = format!("{capability} = {state}");
		assert!(
			capabilities.lines().any(|line| line == expected),
			"{capabilities}"
		);
	}
}

#[cfg(not(any(feature = "signing-native", feature = "signing-rust")))]
#[test]
fn refuses_disabled_signing_before_accessing_the_artifact() {
	let directory = tempfile::tempdir().unwrap();
	let missing = directory.path().join(MISSING_APP);
	for operation in [INSPECT, VERIFY] {
		let output = sign(operation)
			.arg(&missing)
			.arg("--json")
			.output()
			.unwrap();
		let diagnostic = String::from_utf8_lossy(&output.stderr);
		assert_eq!(output.status.code(), Some(ERROR), "{output:?}");
		assert!(output.stdout.is_empty(), "{output:?}");
		assert!(
			diagnostic.contains(&format!("feature not compiled: {CAPABILITY}")),
			"{diagnostic}"
		);
	}
	assert!(!missing.exists());
}

#[cfg(any(feature = "signing-native", feature = "signing-rust"))]
mod enabled {
	use std::process::Output;

	use serde_json::Value;

	use super::{ERROR, INSPECT, MISSING_APP, VERIFY, sign};

	const SCHEMA_VERSION: u32 = 1;
	const AUTO: &str = "auto";
	const NATIVE: &str = "native";
	const RUST: &str = "rust";

	#[test]
	fn unavailable_backend_reports_without_opening_the_artifact() {
		let directory = tempfile::tempdir().unwrap();
		let missing = directory.path().join(MISSING_APP);
		let native = cfg!(all(feature = "signing-native", target_os = "macos"));
		let rust = cfg!(all(
			feature = "signing-rust",
			any(target_os = "linux", target_os = "macos", windows)
		));
		let unavailable = [(AUTO, !native && !rust), (NATIVE, !native), (RUST, !rust)];
		for operation in [INSPECT, VERIFY] {
			for (backend, _) in unavailable
				.into_iter()
				.filter(|&(_, unavailable)| unavailable)
			{
				let output = sign(operation)
					.arg(&missing)
					.args(["--backend", backend, "--json"])
					.output()
					.unwrap();
				let report = json_report(&output, ERROR, "unavailable");
				assert_eq!(report["operation"], operation);
				assert_eq!(report["requested_backend"], backend);
				assert!(report["backend"].is_null(), "{report}");
				assert!(report["kind"].is_null(), "{report}");
			}
		}
		assert!(!missing.exists());
	}

	fn json_report(output: &Output, exit_code: i32, status: &str) -> Value {
		assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
		assert!(output.stderr.is_empty(), "{output:?}");
		let report: Value = serde_json::from_slice(&output.stdout).unwrap();
		assert_eq!(report["schema_version"], SCHEMA_VERSION);
		assert_eq!(report["status"], status, "{report}");
		assert_eq!(report["assessment"], "not_performed", "{report}");
		report
	}

	#[cfg(all(feature = "signing-native", target_os = "macos"))]
	mod macos {
		use std::{
			collections::BTreeMap,
			fs,
			os::unix::fs::PermissionsExt,
			path::{Path, PathBuf},
			process::{Command, Output},
		};

		use serde_json::Value;

		use super::{AUTO, ERROR, INSPECT, NATIVE, VERIFY, json_report, sign};

		const CODESIGN: &str = "/usr/bin/codesign";
		const XATTR: &str = "/usr/bin/xattr";
		const MODIFY_FIXTURE: [&str; 2] = ["--sign", "-"];
		const DISPLAY_SIGNATURE: [&str; 2] = ["--display", "--verbose=4"];
		const VERIFY_SIGNATURE: [&str; 3] = ["--verify", "--strict", "--all-architectures"];
		const VERIFY_BUNDLE: [&str; 4] = ["--verify", "--strict", "--all-architectures", "--deep"];
		const DMG_BYTES: &[u8] = include_bytes!("../../../tests/fixtures/images/tiny-UDZO.dmg");
		const DMG_IDENTIFIER: &str = "org.dmgez.disk-image-fixture";
		const APP_KIND: &str = "app_bundle";
		const DISK_KIND: &str = "disk_image";
		const SUCCESS: i32 = 0;
		const FINDING: i32 = 1;
		const PRESENT: &str = "present";
		const VERIFIED: &str = "verified";
		const UNSIGNED: &str = "unsigned";
		const FAILED: &str = "failed";
		const AD_HOC: &str = "Signature=adhoc";
		const ESCAPE: char = '\u{1b}';

		#[test]
		fn checks_a_nested_bundle_without_modifying_it() {
			let directory = tempfile::tempdir().unwrap();
			let outer = directory.path().join("Installer.app");
			let inner = outer.join("Contents/Helpers/Helper.app");
			make_bundle(&outer, "Installer");
			let sealed_resource = make_bundle(&inner, "Helper");

			let unsigned = snapshot(&outer);
			assert!(!codesign(&outer, &DISPLAY_SIGNATURE).status.success());
			let report = native_report(&outer, INSPECT, AUTO, FINDING, UNSIGNED);
			assert_eq!(report["kind"], APP_KIND);
			native_report(&outer, VERIFY, NATIVE, FINDING, UNSIGNED);
			assert_eq!(snapshot(&outer), unsigned);

			assert_native_success(&codesign(&inner, &MODIFY_FIXTURE));
			assert_native_success(&codesign(&outer, &MODIFY_FIXTURE));
			let signed = snapshot(&outer);
			assert_ne!(signed, unsigned);
			assert_native_success(&codesign(&outer, &VERIFY_BUNDLE));
			let report = native_report(&outer, INSPECT, AUTO, SUCCESS, PRESENT);
			assert_ad_hoc(&report);
			native_report(&outer, VERIFY, NATIVE, SUCCESS, VERIFIED);
			assert_eq!(snapshot(&outer), signed);

			fs::write(sealed_resource, b"changed nested resource\n").unwrap();
			let tampered = snapshot(&outer);
			assert!(!codesign(&outer, &VERIFY_BUNDLE).status.success());
			native_report(&outer, INSPECT, NATIVE, SUCCESS, PRESENT);
			native_report(&outer, VERIFY, AUTO, FINDING, FAILED);
			assert_eq!(snapshot(&outer), tampered);
		}

		#[test]
		fn checks_a_disk_image_without_modifying_it() {
			let directory = tempfile::tempdir().unwrap();
			let image = directory.path().join("Installer\nFormat=marker.dmg");
			fs::write(&image, DMG_BYTES).unwrap();

			let unsigned = snapshot(&image);
			assert!(!codesign(&image, &DISPLAY_SIGNATURE).status.success());
			let report = native_report(&image, INSPECT, NATIVE, FINDING, UNSIGNED);
			assert_eq!(report["kind"], DISK_KIND);
			assert_eq!(snapshot(&image), unsigned);

			sign_image(&image);
			let signed = snapshot(&image);
			let display = codesign(&image, &DISPLAY_SIGNATURE);
			assert_native_success(&display);
			assert!(String::from_utf8_lossy(&display.stderr).contains("Format=disk image"));
			assert_native_success(&codesign(&image, &VERIFY_SIGNATURE));
			let report = native_report(&image, INSPECT, AUTO, SUCCESS, PRESENT);
			assert_ad_hoc(&report);
			native_report(&image, VERIFY, NATIVE, SUCCESS, VERIFIED);
			assert_eq!(snapshot(&image), signed);

			let mut altered_bytes = fs::read(&image).unwrap();
			altered_bytes[0] ^= 1;
			fs::write(&image, altered_bytes).unwrap();
			let tampered = snapshot(&image);
			assert_native_success(&codesign(&image, &DISPLAY_SIGNATURE));
			assert!(!codesign(&image, &VERIFY_SIGNATURE).status.success());
			native_report(&image, INSPECT, NATIVE, SUCCESS, PRESENT);
			native_report(&image, VERIFY, NATIVE, FINDING, FAILED);
			assert_eq!(snapshot(&image), tampered);
		}

		#[test]
		fn refuses_to_call_generic_signed_data_a_verified_disk_image() {
			const GENERIC_BYTES: &[u8] = b"signed generic fixture\n";
			const SHORT_GENERIC_BYTES: &[u8] = b"foo\n";
			const UDIF_TRAILER_BYTES: usize = 512;
			const HEADER_ERROR: &str = "disk image lacks an admitted UDIF v4 trailer header";
			const FORMAT_ERROR: &str =
				"native signature tool did not identify the artifact as a disk image";
			let directory = tempfile::tempdir().unwrap();
			let trailer = DMG_BYTES.last_chunk::<UDIF_TRAILER_BYTES>().unwrap();
			let misleading_payload = [SHORT_GENERIC_BYTES, trailer.as_slice()].concat();
			for (name, bytes, expected) in [
				("Generic.dmg", GENERIC_BYTES, HEADER_ERROR),
				(
					"Generic with UDIF trailer.dmg",
					&misleading_payload,
					FORMAT_ERROR,
				),
			] {
				let fake_image = directory.path().join(name);
				fs::write(&fake_image, bytes).unwrap();
				sign_image(&fake_image);
				let display = codesign(&fake_image, &DISPLAY_SIGNATURE);
				assert_native_success(&display);
				let metadata = String::from_utf8_lossy(&display.stderr);
				assert!(
					metadata.lines().any(|line| line == "Format=generic"),
					"{metadata}"
				);
				assert_native_success(&codesign(&fake_image, &VERIFY_SIGNATURE));
				let signed = snapshot(&fake_image);
				for operation in [INSPECT, VERIFY] {
					let output = sign(operation)
						.arg(&fake_image)
						.args(["--backend", NATIVE, "--json"])
						.output()
						.unwrap();
					assert_eq!(output.status.code(), Some(ERROR), "{output:?}");
					assert!(output.stdout.is_empty(), "{output:?}");
					let diagnostic = String::from_utf8_lossy(&output.stderr);
					assert!(diagnostic.contains(expected), "{diagnostic}");
				}
				assert_eq!(snapshot(&fake_image), signed);
			}
		}

		#[test]
		fn passes_a_literal_path_and_escapes_tool_diagnostics() {
			const APP_NAME: &str = "--example with spaces\n\u{1b}[31m.app";
			let directory = tempfile::tempdir().unwrap();
			let app = directory.path().join(APP_NAME);
			make_bundle(&app, "Escaped");
			assert_native_success(&codesign(&app, &MODIFY_FIXTURE));
			let signed = snapshot(&app);

			let output = sign(INSPECT)
				.current_dir(directory.path())
				.args(["--backend", NATIVE, "--", APP_NAME])
				.output()
				.unwrap();
			assert_eq!(output.status.code(), Some(SUCCESS), "{output:?}");
			assert!(output.stderr.is_empty(), "{output:?}");
			let rendered = String::from_utf8(output.stdout).unwrap();
			assert!(!rendered.contains(ESCAPE), "{rendered:?}");
			assert!(rendered.contains("\\n"), "{rendered:?}");
			assert!(rendered.contains(AD_HOC), "{rendered:?}");
			assert_eq!(
				rendered
					.lines()
					.filter(|line| line.starts_with("diagnostics = "))
					.count(),
				1,
				"{rendered:?}"
			);

			let output = sign(INSPECT)
				.current_dir(directory.path())
				.args(["--backend", NATIVE, "--json", "--", APP_NAME])
				.output()
				.unwrap();
			assert!(!String::from_utf8_lossy(&output.stdout).contains(ESCAPE));
			let report = json_report(&output, SUCCESS, PRESENT);
			assert!(report["artifact"].as_str().unwrap().ends_with(APP_NAME));
			assert_eq!(snapshot(&app), signed);
		}

		fn native_report(
			path: &Path,
			operation: &str,
			requested_backend: &str,
			exit_code: i32,
			status: &str,
		) -> Value {
			let output = sign(operation)
				.arg(path)
				.args(["--backend", requested_backend, "--json"])
				.output()
				.unwrap();
			let report = json_report(&output, exit_code, status);
			assert_eq!(report["operation"], operation);
			assert_eq!(report["requested_backend"], requested_backend);
			assert_eq!(report["backend"], NATIVE);
			report
		}

		fn assert_ad_hoc(report: &Value) {
			let diagnostics = report["diagnostics"].as_str().unwrap();
			assert!(diagnostics.contains(AD_HOC), "{diagnostics}");
		}

		fn make_bundle(app: &Path, executable: &str) -> PathBuf {
			const SCRIPT_BYTES: &[u8] = b"#!/bin/sh\nexit 0\n";
			const EXECUTABLE_PERMISSIONS: u32 = 0o755;
			let contents = app.join("Contents");
			let executable_dir = contents.join("MacOS");
			let resource_dir = contents.join("Resources");
			fs::create_dir_all(&executable_dir).unwrap();
			fs::create_dir(&resource_dir).unwrap();
			let script = executable_dir.join(executable);
			fs::write(&script, SCRIPT_BYTES).unwrap();
			fs::set_permissions(&script, fs::Permissions::from_mode(EXECUTABLE_PERMISSIONS))
				.unwrap();
			fs::write(
				contents.join("Info.plist"),
				format!(
					r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
<key>CFBundleExecutable</key><string>{executable}</string>
<key>CFBundleIdentifier</key><string>org.dmgez.fixture.{executable}</string>
<key>CFBundleName</key><string>{executable}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
</dict>
</plist>"#
				),
			)
			.unwrap();
			let resource = resource_dir.join("fixture.txt");
			fs::write(&resource, b"synthetic sealed resource\n").unwrap();
			resource
		}

		fn codesign(path: &Path, arguments: &[&str]) -> Output {
			Command::new(CODESIGN)
				.env_clear()
				.env("LC_ALL", "C")
				.args(arguments)
				.arg("--")
				.arg(path)
				.output()
				.unwrap()
		}

		fn sign_image(path: &Path) {
			assert_native_success(&codesign(path, &["--sign", "-", "-i", DMG_IDENTIFIER]));
		}

		fn assert_native_success(output: &Output) {
			assert!(output.status.success(), "{output:?}");
		}

		#[derive(Debug, PartialEq, Eq)]
		struct Snapshot {
			entries: BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>,
			attributes: Vec<u8>,
		}

		fn snapshot(path: &Path) -> Snapshot {
			let mut entries = BTreeMap::new();
			record_entries(path, &mut entries);
			let attributes = Command::new(XATTR)
				.args(["-xrl", "--"])
				.arg(path)
				.output()
				.unwrap();
			assert_native_success(&attributes);
			Snapshot {
				entries,
				attributes: attributes.stdout,
			}
		}

		fn record_entries(path: &Path, entries: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
			let metadata = path.symlink_metadata().unwrap();
			let permissions = metadata.permissions().mode();
			let contents = if metadata.is_dir() {
				for entry in fs::read_dir(path).unwrap() {
					record_entries(&entry.unwrap().path(), entries);
				}
				None
			} else {
				assert!(metadata.is_file(), "{path:?}");
				Some(fs::read(path).unwrap())
			};
			entries.insert(path.to_owned(), (permissions, contents));
		}
	}
}
