use std::{fs, os::unix::fs::symlink};

use super::*;

fn snapshot(path: &Path) -> SourceSnapshot {
	let anchor = host::Anchor::new().unwrap();
	let leaf = anchor.locate(path).unwrap();
	anchor
		.snapshot(&leaf.directory, leaf.name, path.into())
		.unwrap()
}

fn signed_directory(path: &Path) -> PathBuf {
	let signature = path.join(CODE_SIGNATURE_DIRECTORY);
	fs::create_dir_all(&signature).unwrap();
	fs::write(signature.join(CODE_RESOURCES), b"marker").unwrap();
	signature
}

#[test]
fn a_marker_protects_an_extensionless_app_and_its_nested_source() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let bundle = root.join("Renamed");
	signed_directory(&bundle.join(BUNDLE_CONTENTS));
	let file = bundle.join(BUNDLE_CONTENTS).join("document");
	fs::write(&file, b"source bytes").unwrap();
	for path in [&bundle, &file] {
		assert!(snapshot(path).has_signed_bundle_marker().unwrap());
	}
}

#[test]
fn a_contents_symlink_is_not_followed_to_a_marker() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let folder = root.join("Folder");
	let outside = root.join("Unrelated");
	signed_directory(&outside);
	fs::create_dir_all(&folder).unwrap();
	symlink(&outside, folder.join(BUNDLE_CONTENTS)).unwrap();
	assert!(!snapshot(&folder).has_signed_bundle_marker().unwrap());
}

#[test]
fn a_flat_signature_protects_a_selected_source_nested_below_it() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let bundle = root.join("FlatBundle");
	signed_directory(&bundle);
	let resources = bundle.join("Nested").join("Resources");
	fs::create_dir_all(&resources).unwrap();
	let file = resources.join("document");
	fs::write(&file, b"sealed resource").unwrap();
	assert!(snapshot(&file).has_signed_bundle_marker().unwrap());
}

#[test]
fn a_relative_parent_does_not_add_the_left_behind_directory_as_an_ancestor() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let bundle = root.join("FlatBundle");
	signed_directory(&bundle);
	fs::write(root.join("document"), b"ordinary file").unwrap();
	let source_path = bundle.join("..").join("document");
	assert!(!snapshot(&source_path).has_signed_bundle_marker().unwrap());
}

#[test]
fn a_physical_version_protects_standard_and_renamed_framework_roots() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let framework = root.join("Sample.framework");
	let versions = framework.join(FRAMEWORK_VERSIONS);
	signed_directory(&versions.join("A"));
	symlink("A", versions.join("Current")).unwrap();
	assert!(snapshot(&framework).has_signed_bundle_marker().unwrap());
	assert!(snapshot(&versions).has_signed_bundle_marker().unwrap());

	let renamed = root.join("RenamedFramework");
	fs::rename(framework, &renamed).unwrap();
	assert!(snapshot(&renamed).has_signed_bundle_marker().unwrap());
}

#[test]
fn framework_detection_skips_symlinked_version_paths() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let outside = root.join("Unrelated");
	let outside_versions = outside.join(FRAMEWORK_VERSIONS);
	let outside_version = outside_versions.join("A");
	signed_directory(&outside_version);

	let symlinked_versions = root.join("SymlinkedVersions");
	fs::create_dir_all(&symlinked_versions).unwrap();
	symlink(
		&outside_versions,
		symlinked_versions.join(FRAMEWORK_VERSIONS),
	)
	.unwrap();
	assert!(
		!snapshot(&symlinked_versions)
			.has_signed_bundle_marker()
			.unwrap()
	);

	let unsigned = root.join("Unsigned");
	let versions = unsigned.join(FRAMEWORK_VERSIONS);
	fs::create_dir_all(versions.join("A")).unwrap();
	fs::write(versions.join("A").join("document"), b"data").unwrap();
	assert!(!snapshot(&unsigned).has_signed_bundle_marker().unwrap());
	symlink(&outside_version, versions.join("Current")).unwrap();
	assert!(!snapshot(&unsigned).has_signed_bundle_marker().unwrap());
}

#[test]
fn a_marker_must_be_a_regular_file() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let framework = root.join("FrameworkLookalike");
	let version = framework.join(FRAMEWORK_VERSIONS).join("A");
	let signature = version.join(CODE_SIGNATURE_DIRECTORY);
	let marker = signature.join(CODE_RESOURCES);
	fs::create_dir_all(&marker).unwrap();
	assert!(!snapshot(&framework).has_signed_bundle_marker().unwrap());

	fs::remove_dir(&marker).unwrap();
	let outside = root.join("Unrelated");
	let outside_marker = signed_directory(&outside).join(CODE_RESOURCES);
	symlink(outside_marker, marker).unwrap();
	assert!(!snapshot(&framework).has_signed_bundle_marker().unwrap());
}

#[test]
fn framework_detection_bounds_the_number_of_entries_examined() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	let folder = root.join("Folder");
	let versions = folder.join(FRAMEWORK_VERSIONS);
	fs::create_dir_all(&versions).unwrap();
	for index in 0..MAX_FRAMEWORK_VERSION_ENTRIES {
		fs::write(versions.join(index.to_string()), b"data").unwrap();
	}
	assert!(!snapshot(&folder).has_signed_bundle_marker().unwrap());
	fs::write(versions.join("additional"), b"data").unwrap();
	assert!(matches!(
		snapshot(&folder).has_signed_bundle_marker(),
		Err(Error::Limit(Budget::Visited))
	));
}
