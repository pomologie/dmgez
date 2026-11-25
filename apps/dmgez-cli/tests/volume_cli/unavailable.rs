use super::invoke;

const BROWSE: [&str; 4] = ["ls", "tree", "stat", "cat"];
const ROOT_PATH: &str = "/";

#[cfg(not(feature = "hfsplus-read"))]
#[test]
fn default_volume_commands_report_disabled_hfs_backend_before_opening_input() {
	const ARGUMENTS: &[&str] = &[ROOT_PATH];
	assert_unavailable(BROWSE.map(|command| (command, ARGUMENTS)), "hfsplus-read");
}

#[cfg(not(feature = "apfs-read"))]
#[test]
fn apfs_commands_report_disabled_apfs_backend_before_opening_input() {
	const ARGUMENTS: &[&str] = &[ROOT_PATH, "--filesystem", "apfs"];
	let commands = BROWSE
		.map(|command| (command, ARGUMENTS))
		.into_iter()
		.chain([("volumes", [].as_slice())]);
	assert_unavailable(commands, "apfs-read");
}

fn assert_unavailable<'a>(
	commands: impl IntoIterator<Item = (&'a str, &'a [&'a str])>,
	feature: &str,
) {
	let directory = tempfile::tempdir().unwrap();
	let absent = directory.path().join("absent.dmg");
	let expected = format!("feature not compiled: {feature}");
	for (command, options) in commands {
		let output = invoke(command, &absent, options);
		assert!(!output.status.success(), "{command}");
		assert!(output.stdout.is_empty(), "{command}");
		let diagnostic = String::from_utf8_lossy(&output.stderr);
		assert!(diagnostic.contains(&expected), "{command}: {diagnostic}");
	}
	assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
