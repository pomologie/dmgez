use std::{fs, path::Path};

use super::{FileAdmission, LocalRoot};

const PROJECT_FILE: &str = "captured.toml";
const ORIGINAL_FILE: &str = "original.tmp";
const ORIGINAL_CONTENTS: &[u8] = b"original";
const REPLACEMENT_CONTENTS: &[u8] = b"modified";

#[test]
fn rejects_a_replacement_of_the_admitted_named_file() {
	let temp = tempfile::tempdir().unwrap();
	let physical = temp.path().canonicalize().unwrap();
	let path = physical.join(PROJECT_FILE);
	fs::write(&path, ORIGINAL_CONTENTS).unwrap();
	let root = LocalRoot::open(&physical).unwrap();
	let (_file, binding) = root.open_file(Path::new(PROJECT_FILE)).unwrap();
	binding.verify().unwrap();

	fs::rename(&path, physical.join(ORIGINAL_FILE)).unwrap();
	fs::write(path, REPLACEMENT_CONTENTS).unwrap();
	assert!(matches!(binding.verify(), Err(FileAdmission::Changed)));
}
