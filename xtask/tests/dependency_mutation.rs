use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Output};

const CLI_MANIFEST: &str = "apps/dmgez-cli/Cargo.toml";
const DEPENDENCIES_HEADER: &str = "[dependencies]\n";
const GUI_DEPENDENCY: &str = "eframe.workspace = true\n";
const REQUIRED_PROFILES: [&str; 3] = ["minimal", "serial", "default"];
const WORKSPACE_DIRECTORIES: [&str; 5] = ["apps", "crates", "fuzz", "third_party", "xtask"];
const WORKSPACE_FILES: [&str; 2] = ["Cargo.toml", "Cargo.lock"];

#[test]
fn rejects_a_gui_runtime_added_to_the_cli() -> io::Result<()> {
	let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
	let temporary = tempfile::Builder::new()
		.prefix("dmgez-architecture-mutation.")
		.tempdir()?;
	let workspace = temporary.path().join("workspace");
	fs::create_dir(&workspace)?;
	for directory in WORKSPACE_DIRECTORIES {
		copy_source_tree(&source.join(directory), &workspace.join(directory))?;
	}
	for file in WORKSPACE_FILES {
		fs::copy(source.join(file), workspace.join(file))?;
	}

	let manifest = workspace.join(CLI_MANIFEST);
	let source = fs::read_to_string(&manifest)?.replace("\r\n", "\n");
	assert_eq!(source.matches(DEPENDENCIES_HEADER).count(), 1);
	fs::write(
		manifest,
		source.replacen(
			DEPENDENCIES_HEADER,
			&format!("{DEPENDENCIES_HEADER}{GUI_DEPENDENCY}"),
			1,
		),
	)?;

	let resolve = Command::new(env!("CARGO"))
		.args(["update", "--workspace", "--offline"])
		.current_dir(&workspace)
		.output()?;
	assert!(
		resolve.status.success(),
		"Cargo could not resolve the mutated workspace\n{}",
		diagnostics(&resolve)
	);

	let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
		.args(["architecture", "--manifest-path"])
		.arg(workspace.join("Cargo.toml"))
		.env("CARGO", env!("CARGO"))
		.env("CARGO_NET_OFFLINE", "true")
		.current_dir(temporary.path())
		.output()?;
	assert_eq!(result.status.code(), Some(1), "{}", diagnostics(&result));
	let stderr = String::from_utf8_lossy(&result.stderr);
	for profile in REQUIRED_PROFILES {
		let expected = format!("{profile} profile: forbidden dependencies");
		assert!(
			stderr
				.lines()
				.any(|line| line.contains(&expected) && line.contains("'eframe'")),
			"{profile} profile did not report eframe\n{}",
			diagnostics(&result)
		);
	}
	Ok(())
}

fn copy_source_tree(source: &Path, destination: &Path) -> io::Result<()> {
	fs::create_dir(destination)?;
	for entry in fs::read_dir(source)? {
		let entry = entry?;
		let name = entry.file_name();
		let path = destination.join(&name);
		let kind = entry.file_type()?;
		if kind.is_dir() && name != "target" {
			copy_source_tree(&entry.path(), &path)?;
		} else if kind.is_file() {
			fs::copy(entry.path(), path)?;
		}
	}
	Ok(())
}

fn diagnostics(output: &Output) -> String {
	format!(
		"{}{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	)
}
