#![cfg(all(feature = "project-build", feature = "inspect"))]

use std::fs;

use dmgez::{
	dmgbuilder::{Mode, Outcome, Project},
	reader::Dmg,
};

const EXAMPLE: &str = include_str!("../../../examples/offline-cli/dmg.toml");
const PAYLOAD: &str = include_str!("../../../examples/offline-cli/hello.txt");

#[test]
fn plans_builds_and_reads_a_project_through_the_public_api() {
	let directory = tempfile::tempdir().unwrap();
	let manifest = directory.path().join("dmg.toml");
	fs::write(&manifest, EXAMPLE).unwrap();
	fs::write(directory.path().join("hello.txt"), PAYLOAD).unwrap();

	let project = Project::open(&manifest).unwrap();
	let Outcome::Planned {
		output: Some(path),
		logical_bytes,
		..
	} = project.run(Mode::Plan).unwrap()
	else {
		panic!("expected a file-backed plan");
	};
	assert!(!path.exists());
	let Outcome::Built {
		output: Some(output),
		physical_bytes,
		..
	} = project.run(Mode::Build).unwrap()
	else {
		panic!("expected a published image");
	};
	assert_eq!(output, path);
	let mut image = Dmg::open(&output).unwrap();
	assert_eq!(image.logical_bytes(), logical_bytes);
	assert_eq!(image.physical_bytes(), physical_bytes);
	let checks = image.verify().unwrap();
	assert!(checks.data_fork && checks.master);
	assert!(!checks.tables.is_empty() && checks.tables.iter().all(|&checked| checked));
	let mut first_byte = [0];
	image.read_exact_at(0, &mut first_byte).unwrap();
	assert!(image.read_exact_at(logical_bytes, &mut [0]).is_err());
}
