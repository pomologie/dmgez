use std::process::{Command, Output};

const DMGEZ: &str = env!("CARGO_BIN_EXE_dmgez");
const DOCUMENT: &str = "installer.dmg";

fn failure(output: &Output, message: &str) {
	assert!(!output.status.success());
	let stderr = String::from_utf8_lossy(&output.stderr);
	assert!(stderr.contains(message), "{stderr}");
}

#[test]
fn unavailable_desktop_has_a_diagnostic() {
	let directory = tempfile::tempdir().unwrap();
	let document = directory.path().join(DOCUMENT);
	std::fs::write(&document, []).unwrap();
	let output = Command::new(DMGEZ)
		.arg("open")
		.arg(&document)
		.arg("--gui")
		.arg(directory.path().join("missing"))
		.output()
		.unwrap();
	let expected = if cfg!(feature = "desktop-open") {
		"desktop executable unavailable"
	} else {
		"feature not compiled: desktop-open"
	};
	failure(&output, expected);
}

#[cfg(all(unix, feature = "desktop-open"))]
mod handoff {
	use std::{
		fs,
		os::unix::{ffi::OsStrExt, fs::PermissionsExt},
		path::Path,
		thread,
		time::{Duration, Instant},
	};

	use super::{Command, DMGEZ, failure};

	const GUI: &str = "dmgez-gui";
	const CAPTURE: &str = "captured";
	const TIMEOUT: Duration = Duration::from_secs(5);
	const POLL_INTERVAL: Duration = Duration::from_millis(10);
	const RECEIVER: &str = r#"#!/bin/sh
base=${0%/*}
printf '%s\000' "$@" > "$base/temporary"
mv "$base/temporary" "$base/captured"
"#;

	fn receiver(path: &Path) {
		fs::write(path, RECEIVER).unwrap();
		fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
	}

	fn captured(path: &Path) -> Vec<u8> {
		let deadline = Instant::now() + TIMEOUT;
		loop {
			match fs::read(path) {
				Ok(bytes) => return bytes,
				Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
					assert!(
						Instant::now() < deadline,
						"desktop process did not receive its argument"
					);
					thread::sleep(POLL_INTERVAL);
				}
				Err(error) => panic!("could not read the desktop capture: {error}"),
			}
		}
	}

	#[test]
	fn forwards_one_literal_path_to_an_explicit_or_adjacent_executable() {
		let directory = tempfile::tempdir().unwrap();
		let cwd = directory.path().canonicalize().unwrap();
		let name = "-installer & $HOME's project; with spaces.dmg";
		let document = cwd.join(name);
		fs::write(&document, []).unwrap();
		let explicit = cwd.join("test desktop app");
		receiver(&explicit);
		let adjacent = cwd.join(GUI);
		receiver(&adjacent);
		let installed = cwd.join("dmgez");
		fs::copy(DMGEZ, &installed).unwrap();
		let mut expected = document.as_os_str().as_bytes().to_vec();
		expected.push(0);
		for (program, option) in [
			(Path::new(DMGEZ), Some(&explicit)),
			(installed.as_path(), None),
		] {
			let mut command = Command::new(program);
			command.current_dir(&cwd).arg("open");
			if let Some(gui) = option {
				command.arg("--gui").arg(gui);
			}
			let output = command.arg("--").arg(name).output().unwrap();
			assert!(output.status.success(), "{output:?}");
			let capture = cwd.join(CAPTURE);
			assert_eq!(captured(&capture), expected);
			fs::remove_file(capture).unwrap();
		}
	}

	#[test]
	fn rejects_directory_inputs_before_starting_the_desktop() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path();
		let desktop = path.join(GUI);
		receiver(&desktop);
		let output = Command::new(DMGEZ)
			.arg("open")
			.arg("--gui")
			.arg(desktop)
			.arg(path)
			.output()
			.unwrap();
		failure(&output, "requested document is not a regular file");
	}
}

#[cfg(all(windows, feature = "desktop-open"))]
#[test]
fn declines_shell_script_as_desktop_program() {
	let directory = tempfile::tempdir().unwrap();
	let script = directory.path().join("desktop.cmd");
	let document = directory.path().join(DOCUMENT);
	std::fs::write(&script, "@exit /b 0").unwrap();
	std::fs::write(&document, []).unwrap();
	let output = Command::new(DMGEZ)
		.arg("open")
		.arg(&document)
		.arg("--gui")
		.arg(&script)
		.output()
		.unwrap();
	failure(&output, "not a supported executable file");
}
