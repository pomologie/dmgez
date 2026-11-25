use std::process::Command;

use hfsplus_format::volume::CatalogId;

use super::{
	Node, Project,
	background::{folder, publish_project, read_fork, store_fork},
	first_leaf, fs, open_output,
};

const CONFIG: &str = include_str!("../fixtures/finder-column.toml");
const ORACLE: &str = include_str!("column_view_oracle.py");
const IMAGE: &str = "column-layout.dmg";
const STORE: &str = ".DS_Store";
const ROOT: CatalogId = CatalogId(2);
const DOCUMENTATION: &str = "Documentation";
const CHANGELOG: &str = "Changelog";

#[test]
#[ignore = "requires Python ds_store 1.3.4"]
fn independent_oracle_matches_generated_column_view_stores() {
	let scratch = tempfile::tempdir().unwrap();
	let root = scratch.path().canonicalize().unwrap();
	publish_project(&root, &Project::parse(CONFIG).unwrap());
	let (mut image, header) = open_output(&root.join(IMAGE));
	let catalog = first_leaf(&mut image, &header.forks.catalog, header.geometry);
	let node = Node::decode(&catalog).unwrap();
	let documentation = folder(&node, &header, ROOT, DOCUMENTATION).id;
	let changelog = folder(&node, &header, ROOT, CHANGELOG).id;
	for (directory, parent) in [
		(root.clone(), ROOT),
		(root.join(DOCUMENTATION), documentation),
		(root.join(CHANGELOG), changelog),
	] {
		fs::create_dir_all(&directory).unwrap();
		let fork = store_fork(&node, &header, parent);
		fs::write(directory.join(STORE), read_fork(&mut image, &header, &fork)).unwrap();
	}
	let result = Command::new("python3")
		.args(["-B", "-c", ORACLE])
		.arg(&root)
		.output()
		.unwrap();
	assert!(
		result.status.success(),
		"{}",
		String::from_utf8_lossy(&result.stderr)
	);
}
