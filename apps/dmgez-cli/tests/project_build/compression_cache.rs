use std::{
	fs,
	os::unix::fs::PermissionsExt,
	path::{Path, PathBuf},
	process::Output,
};

use super::{BINARY, Command, MANIFEST_NAME};

const CACHE_DIRECTORY: &str = "cache";
const SOURCE: &str = "payload.bin";
const OUTPUT: &str = "installer.dmg";
const OWNER_ONLY: u32 = 0o700;
const PAYLOAD_BYTES: usize = 256 * 1024;

const SETTINGS: &str = r#"
[build]
reproducible = true
source_date_epoch = 1700000000
[metadata]
policy = "report"
preserve_timestamps = false
[compression]
block_size = "64KiB"
"#;

fn private_directory(root: &Path, directory: &str) -> PathBuf {
	let path = root.join(directory);
	fs::create_dir_all(&path).unwrap();
	fs::set_permissions(&path, fs::Permissions::from_mode(OWNER_ONLY)).unwrap();
	path
}

fn write_project(root: &Path, cache: &str, source: &str, output: &str) {
	let project = format!(
		r#"
schema_version = 1
[image]
name = "Cache Fixture"
format = "udzo"
size = "4MiB"
output = "{output}"
{SETTINGS}
[performance]
parallel = false
[performance.cache]
directory = "{cache}"
max_bytes = "4MiB"
[[contents]]
source = "{source}"
destination = "/Payload"
"#
	);
	fs::write(root.join(MANIFEST_NAME), project).unwrap();
}

fn build(root: &Path, options: &[&str]) -> Output {
	Command::new(BINARY)
		.current_dir(root)
		.args(["build", MANIFEST_NAME])
		.args(options)
		.output()
		.unwrap()
}

fn write_payload(path: &Path) {
	const PAYLOAD_BYTE: u8 = 0x31;
	fs::write(path, vec![PAYLOAD_BYTE; PAYLOAD_BYTES]).unwrap();
}

fn assert_refused(root: &Path, diagnostic: &str) {
	let output = build(root, &[]);
	assert!(!output.status.success(), "{output:?}");
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(stderr.contains(diagnostic), "{stderr}");
	assert!(output.stdout.is_empty());
}

#[cfg(not(feature = "compression-cache"))]
#[test]
fn refuses_cache_configuration_without_the_compiled_feature() {
	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let cache = private_directory(root, CACHE_DIRECTORY);
	write_payload(&root.join(SOURCE));
	write_project(root, CACHE_DIRECTORY, SOURCE, OUTPUT);

	assert_refused(
		root,
		"feature not compiled: compression cache (compression-cache)",
	);
	assert!(!root.join(OUTPUT).exists());
	assert_eq!(fs::read_dir(cache).unwrap().count(), 0);
}

#[cfg(feature = "compression-cache")]
#[test]
fn cache_hits_preserve_the_output_and_corrupt_entries_are_regenerated() {
	const WARM_OUTPUT: &str = "warm.dmg";
	const RECOVERED_OUTPUT: &str = "recovered.dmg";
	const HIT: &str = "cache_hits";
	const MISS: &str = "cache_misses";
	const REJECTED: &str = "cache_rejected";

	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let cache = private_directory(root, CACHE_DIRECTORY);
	write_payload(&root.join(SOURCE));
	write_project(root, CACHE_DIRECTORY, SOURCE, OUTPUT);

	let dry_run = build(root, &["--dry-run"]);
	assert!(dry_run.status.success(), "{dry_run:?}");
	assert!(!root.join(OUTPUT).exists());
	assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);

	let cold = build(root, &[]);
	assert!(statistic(cold, MISS) > 0);
	let original = fs::read(root.join(OUTPUT)).unwrap();
	assert!(!original.is_empty());
	let entries = cache_entries(&cache);
	assert!(!entries.is_empty());

	let warm = build(root, &["--set", &format!("image.output={WARM_OUTPUT}")]);
	assert!(statistic(warm, HIT) > 0);
	assert_eq!(fs::read(root.join(WARM_OUTPUT)).unwrap(), original);

	for path in &entries {
		let length = fs::read(path).unwrap().len();
		assert!(length > 0);
		fs::write(path, vec![0; length]).unwrap();
	}
	let recovered = build(
		root,
		&["--set", &format!("image.output={RECOVERED_OUTPUT}")],
	);
	assert!(statistic(recovered, REJECTED) > 0);
	assert_eq!(fs::read(root.join(RECOVERED_OUTPUT)).unwrap(), original);
	assert_eq!(cache_entries(&cache), entries);
	for path in entries {
		assert!(fs::read(path).unwrap().iter().any(|&byte| byte != 0));
	}
}

#[cfg(feature = "compression-cache")]
#[test]
fn refuses_cache_overlap_with_copied_trees_explicit_inputs_and_output() {
	const COPIED_TREE: &str = "copied";
	const NESTED_CACHE: &str = "copied/cache";
	const SOURCE_INSIDE_CACHE: &str = "cache/payload.bin";
	const OUTPUT_INSIDE_CACHE: &str = "cache/installer.dmg";
	const DIAGNOSTIC: &str = "compression cache overlaps a build source or output";

	let directory = tempfile::tempdir().unwrap();
	let root = directory.path();
	let nested_cache = private_directory(root, NESTED_CACHE);
	write_payload(&root.join(COPIED_TREE).join(SOURCE));
	write_project(root, NESTED_CACHE, COPIED_TREE, OUTPUT);
	assert_refused(root, DIAGNOSTIC);
	assert!(!root.join(OUTPUT).exists());
	assert_eq!(fs::read_dir(nested_cache).unwrap().count(), 0);

	let cache = private_directory(root, CACHE_DIRECTORY);
	let input = root.join(SOURCE_INSIDE_CACHE);
	write_payload(&input);
	let original = fs::read(&input).unwrap();
	write_project(root, CACHE_DIRECTORY, SOURCE_INSIDE_CACHE, OUTPUT);
	assert_refused(root, DIAGNOSTIC);
	assert!(!root.join(OUTPUT).exists());
	assert_eq!(fs::read(&input).unwrap(), original);
	assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);

	write_payload(&root.join(SOURCE));
	write_project(root, CACHE_DIRECTORY, SOURCE, OUTPUT_INSIDE_CACHE);
	assert_refused(root, DIAGNOSTIC);
	assert!(!root.join(OUTPUT_INSIDE_CACHE).exists());
	assert_eq!(fs::read(&input).unwrap(), original);
	assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
}

#[cfg(feature = "compression-cache")]
fn statistic(output: Output, name: &str) -> usize {
	let stderr = String::from_utf8(output.stderr).unwrap();
	assert!(output.status.success(), "{stderr}");
	let prefix = format!("{name}=");
	stderr
		.lines()
		.find_map(|line| line.strip_prefix(&prefix))
		.unwrap_or_else(|| panic!("missing {name} in build report: {stderr}"))
		.parse()
		.unwrap()
}

#[cfg(feature = "compression-cache")]
fn cache_entries(directory: &Path) -> Vec<PathBuf> {
	const ENTRY_PREFIX: &str = "block-v1-";
	let mut paths: Vec<_> = fs::read_dir(directory)
		.unwrap()
		.map(|entry| entry.unwrap())
		.filter(|entry| {
			entry
				.file_name()
				.to_string_lossy()
				.starts_with(ENTRY_PREFIX)
		})
		.map(|entry| entry.path())
		.collect();
	paths.sort();
	paths
}
