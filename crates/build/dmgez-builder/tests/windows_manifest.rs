#![cfg(windows)]

use std::{fs, io::Read, path::Path, process::Command};

use dmgez_builder::manifest::{Budget, EntryKind, Error, Limits, discover};
use dmgez_config::Project;

const BASE: &str = "schema_version=1\n[image]\nname='Windows evidence'\noutput='out.dmg'\n";
const COPY_FILE: &str = "\n[[contents]]\nsource='source/payload.txt'\ndestination='/payload.txt'\n";
const COPY_TREE: &str = "\n[[contents]]\nsource='source'\ndestination='/source'\n";
const COPY_FILTERED: &str = "\n[[contents]]\nsource='source'\ndestination='/source'\ninclude=['nested/*.txt']\nexclude=['nested/ignore.txt']\n";
const PAYLOAD: &[u8] = b"ordinary NTFS payload";

fn project(contents: &str) -> Project {
	Project::parse(&format!("{BASE}{contents}")).unwrap()
}

fn write_source(root: &Path) {
	fs::create_dir(root.join("source")).unwrap();
	fs::write(root.join("source/payload.txt"), PAYLOAD).unwrap();
}

#[test]
fn preserves_the_ordinary_tree_paths_and_identifies_hard_links() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	fs::create_dir_all(root.join("source/nested")).unwrap();
	fs::create_dir(root.join("source/empty")).unwrap();
	fs::write(root.join("source/nested/payload.txt"), PAYLOAD).unwrap();
	fs::hard_link(
		root.join("source/nested/payload.txt"),
		root.join("source/nested/alias.txt"),
	)
	.unwrap();
	fs::write(root.join("source/nested/ignore.txt"), b"excluded").unwrap();
	fs::write(root.join("source/nested/image.bin"), b"filtered").unwrap();
	let unfiltered = discover(&project(COPY_TREE), &root, Limits::default()).unwrap();
	assert!(
		unfiltered
			.entries()
			.iter()
			.any(|entry| entry.destination.as_str() == "/source/empty"
				&& matches!(entry.kind, EntryKind::Directory))
	);
	let manifest = discover(&project(COPY_FILTERED), &root, Limits::default()).unwrap();
	let paths: Vec<_> = manifest
		.entries()
		.iter()
		.map(|entry| entry.destination.as_str())
		.collect();
	assert_eq!(
		paths,
		[
			"/source",
			"/source/nested",
			"/source/nested/alias.txt",
			"/source/nested/payload.txt"
		]
	);
	let alias = manifest.entries()[2].source.as_ref().unwrap();
	let source = manifest.entries()[3].source.as_ref().unwrap();
	assert_eq!(source.metadata().identity, alias.metadata().identity);
	assert_eq!(source.metadata().link_count, 2);
	assert!(source.metadata().modified.is_some());
	assert!(source.metadata().changed.is_some());
	assert_eq!(source.metadata().permissions, None);
	assert_eq!(source.metadata().ownership, None);
	assert_eq!(source.metadata().flags, None);
	let mut bytes = Vec::new();
	source
		.open_regular()
		.unwrap()
		.read_to_end(&mut bytes)
		.unwrap();
	assert_eq!(bytes, PAYLOAD);
	source.validate().unwrap();
}

#[test]
fn rechecks_original_file_identity_and_rejects_output_aliases() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	write_source(&root);
	fs::hard_link(root.join("source/payload.txt"), root.join("out.dmg")).unwrap();
	assert!(matches!(
		discover(&project(COPY_FILE), &root, Limits::default()),
		Err(Error::OutputSourceAlias { .. })
	));
	fs::remove_file(root.join("out.dmg")).unwrap();
	let project = project(COPY_FILE);
	let manifest = discover(&project, &root, Limits::default()).unwrap();
	let source = manifest.entries()[0].source.as_ref().unwrap();
	fs::rename(root.join("source/payload.txt"), root.join("original.txt")).unwrap();
	fs::write(root.join("source/payload.txt"), PAYLOAD).unwrap();
	assert!(matches!(
		source.open_regular(),
		Err(Error::SourceChanged { .. })
	));
	assert!(matches!(
		source.validate(),
		Err(Error::SourceChanged { .. })
	));
}

#[test]
fn rejects_traversed_junctions_and_junction_entries() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	write_source(&root);
	let manifest = discover(&project(COPY_FILE), &root, Limits::default()).unwrap();
	let source = manifest.entries()[0].source.as_ref().unwrap();
	fs::rename(root.join("source"), root.join("original")).unwrap();
	let link = temp.path().join("source");
	let target = temp.path().join("original");
	let result = Command::new("cmd.exe")
		.args(["/d", "/c", "mklink", "/J"])
		.arg(&link)
		.arg(&target)
		.output()
		.unwrap();
	assert!(
		result.status.success(),
		"could not create test junction: {}",
		String::from_utf8_lossy(&result.stderr)
	);
	assert!(matches!(
		source.open_regular(),
		Err(Error::SourceChanged { .. })
	));
	assert!(discover(&project(COPY_FILE), &root, Limits::default()).is_err());
	assert!(matches!(
		discover(&project(COPY_TREE), &root, Limits::default()),
		Err(Error::UnsupportedSource { .. })
	));
}

#[test]
fn checks_tree_budgets_and_rejects_alternate_stream_names() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	write_source(&root);
	let limits = Limits {
		max_visited: 1,
		..Limits::default()
	};
	assert!(matches!(
		discover(&project(COPY_TREE), &root, limits),
		Err(Error::Limit(Budget::Visited))
	));
	let limits = Limits {
		max_metadata_bytes: 1,
		..Limits::default()
	};
	assert!(matches!(
		discover(&project(COPY_TREE), &root, limits),
		Err(Error::Limit(Budget::Metadata))
	));
	let alternate = "\n[[contents]]\nsource='source/payload.txt:alternate'\ndestination='/data'\n";
	assert!(matches!(
		discover(&project(alternate), &root, Limits::default()),
		Err(Error::Io { .. })
	));
}

#[test]
fn rejects_unqualified_namespaces_and_verbatim_parent_components() {
	let temp = tempfile::tempdir().unwrap();
	let root = temp.path().canonicalize().unwrap();
	write_source(&root);
	let declaration =
		|source: &str| format!("\n[[contents]]\nsource='{source}'\ndestination='/data'\n");
	let ordinary = format!("{}\\source\\..\\source\\payload.txt", temp.path().display());
	assert!(discover(&project(&declaration(&ordinary)), &root, Limits::default()).is_ok());
	let verbatim = format!("{}\\source\\..\\source\\payload.txt", root.display());
	assert!(matches!(
		discover(&project(&declaration(&verbatim)), &root, Limits::default()),
		Err(Error::Io { .. })
	));
	let remote = r"\\example.invalid\share\payload.txt";
	assert!(matches!(
		discover(&project(&declaration(remote)), &root, Limits::default()),
		Err(Error::Io { .. })
	));
}
