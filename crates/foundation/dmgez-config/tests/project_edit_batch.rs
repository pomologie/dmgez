#![cfg(feature = "project-edit")]

use dmgez_config::project_edit::{
	ContentDraft, EditBatchError, FieldChange, MAX_EDIT_BATCH_BYTES, ProjectDocument, ProjectEdit,
	parse_edit_batch,
};

const SCHEMA: &str = "schema_version = 1\n";
const ADD_DIRECTORY: &str = "\n[[edits]]\noperation = 'add_directory'\ndestination = '/Folder'\n";
const MAX_OPERATIONS: usize = 4_096;
const PROJECT: &str = r#"
schema_version = 1

[image]
name = "Installer"
output = "installer.dmg"

[defines]
FILE = "payload.txt"
LABEL = "Revised Installer"

[[contents]]
source = "README.md"
destination = "/Readme.md"
position = [5, 10]
hidden = true
hide_extension = true

[[contents]]
type = "directory"
destination = "/Obsolete"
"#;

const BATCH: &str = r#"
schema_version = 1

[[edits]]
operation = "add_copy"
source = "${FILE}"

[[edits]]
operation = "modify"
target = "/Readme.md"
destination = "/Guide.md"
position = [20, 30.5]
hidden = false
clear_hide_extension = true

[[edits]]
operation = "add_directory"
destination = "/Docs"

[[edits]]
operation = "add_symlink"
destination = "/Link"
target = "Guide.md"

[[edits]]
operation = "remove"
target = "/Obsolete"

[[edits]]
operation = "set_image_name"
name = "${LABEL}"
"#;

#[test]
fn decodes_every_operation_in_order_and_applies_project_substitutions() {
	let edits = parse_edit_batch(BATCH).unwrap();
	let [
		ProjectEdit::Add(ContentDraft::Copy {
			source,
			destination,
		}),
		ProjectEdit::Modify { target, changes },
		ProjectEdit::Add(ContentDraft::Directory {
			destination: directory,
		}),
		ProjectEdit::Add(ContentDraft::Symlink {
			destination: link,
			target: link_target,
		}),
		ProjectEdit::Remove { target: removed },
		ProjectEdit::SetImageName(label),
	] = edits.as_slice()
	else {
		panic!("unexpected edit order: {edits:?}");
	};
	assert_eq!(source, "${FILE}");
	assert!(destination.is_none());
	assert_eq!(target.as_str(), "/Readme.md");
	assert_eq!(changes.destination.as_ref().unwrap().as_str(), "/Guide.md");
	assert_eq!(changes.position, FieldChange::Set([20.0, 30.5]));
	assert_eq!(changes.hidden, FieldChange::Set(false));
	assert_eq!(changes.hide_extension, FieldChange::Clear);
	assert_eq!(directory.as_str(), "/Docs");
	assert_eq!(link.as_str(), "/Link");
	assert_eq!(link_target, "Guide.md");
	assert_eq!(removed.as_str(), "/Obsolete");
	assert_eq!(label, "${LABEL}");

	let mut document = ProjectDocument::open(PROJECT).unwrap();
	document.apply(&edits).unwrap();
	let project = document.project();
	assert_eq!(project.image.name.as_str(), "Revised Installer");
	let paths: Vec<_> = project
		.contents
		.iter()
		.map(|entry| entry.destination.as_str())
		.collect();
	assert_eq!(paths, ["/Guide.md", "/payload.txt", "/Docs", "/Link"]);
	assert!(!document.source().contains("hide_extension"));
	assert!(document.source().contains("source = \"${FILE}\""));
}

#[test]
fn rejects_invalid_schema_unknown_operations_and_untyped_paths() {
	assert_eq!(
		parse_edit_batch("schema_version = 2\nedits = []").unwrap_err(),
		EditBatchError::UnsupportedVersion(2)
	);
	assert_eq!(
		parse_edit_batch("edits = []").unwrap_err(),
		EditBatchError::SchemaVersion
	);
	assert_eq!(
		parse_edit_batch(SCHEMA).unwrap_err(),
		EditBatchError::Schema
	);
	assert_eq!(
		parse_edit_batch(&format!("{SCHEMA}edits = []\noutput = 'copy.dmg'\n")).unwrap_err(),
		EditBatchError::Schema
	);
	assert!(
		parse_edit_batch(&format!("{SCHEMA}edits = []"))
			.unwrap()
			.is_empty()
	);

	for operation in [
		"operation = 'set_output_path'\npath = 'forbidden.dmg'",
		"operation = 'add_directory'\ndestination = 'relative'",
		"operation = 'remove'\ntarget = '/Folder/../Other'",
		"operation = 'remove'\ntarget = '/Folder'\nunexpected = true",
		"operation = 'modify'\ntarget = '/Folder'\nposition = [nan, 3]",
	] {
		let input = format!("{SCHEMA}{ADD_DIRECTORY}\n[[edits]]\n{operation}\n");
		assert_eq!(
			parse_edit_batch(&input).unwrap_err(),
			EditBatchError::Operation { index: 1 },
			"{operation}"
		);
	}
}

#[test]
fn distinguishes_clear_set_and_unchanged_fields() {
	const MODIFY: &str = "[[edits]]\noperation = 'modify'\ntarget = '/Readme.md'\n";
	let input = format!("{SCHEMA}{MODIFY}clear_position = true\nclear_hidden = true\n");
	let edits = parse_edit_batch(&input).unwrap();
	let [ProjectEdit::Modify { changes, .. }] = edits.as_slice() else {
		panic!("unexpected edit: {edits:?}");
	};
	assert_eq!(changes.position, FieldChange::Clear);
	assert_eq!(changes.hidden, FieldChange::Clear);
	assert_eq!(changes.hide_extension, FieldChange::Keep);
	assert_eq!(
		parse_edit_batch(&format!("{SCHEMA}{MODIFY}clear_position = false\n")).unwrap_err(),
		EditBatchError::NoChanges { index: 0 }
	);
	assert_eq!(
		parse_edit_batch(&format!(
			"{SCHEMA}{MODIFY}hidden = false\nclear_hidden = true\n"
		))
		.unwrap_err(),
		EditBatchError::Conflict {
			index: 0,
			field: "hidden",
		}
	);
}

#[test]
fn enforces_both_input_bounds() {
	let oversized = " ".repeat(MAX_EDIT_BATCH_BYTES + 1);
	assert_eq!(
		parse_edit_batch(&oversized).unwrap_err(),
		EditBatchError::TooLarge
	);
	let mut operations = format!("{SCHEMA}{}", ADD_DIRECTORY.repeat(MAX_OPERATIONS));
	assert_eq!(parse_edit_batch(&operations).unwrap().len(), MAX_OPERATIONS);
	operations.push_str(ADD_DIRECTORY);
	assert_eq!(
		parse_edit_batch(&operations).unwrap_err(),
		EditBatchError::TooManyOperations
	);
}
