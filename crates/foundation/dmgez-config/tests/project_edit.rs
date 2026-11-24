//! Project source and reducer coverage for D-GUI-01-T01/T02 and D-V11.

#![cfg(feature = "project-edit")]

use std::collections::BTreeMap;

use dmgez_config::{
	ConfigError, Project, VirtualPath,
	project_edit::{
		ContentChanges, ContentDraft, FieldChange, ProjectDocument, ProjectEdit, ProjectEditError,
	},
};

const PROJECT: &str = "\
# Installer metadata\n\
schema_version = 1\n\
\n\
[image]\n\
'name'  = 'Release' # Keep the name comment\n\
output = 'Release.dmg'\n\
\n\
# Primary app\n\
[[contents]]\n\
source = 'The App.app' # Packaging source\n\
position = [\n\
\t20, # x in points\n\
\t30, # y in points\n\
] # Icon center\n\
\n\
# Documentation folder\n\
[[contents]]\n\
type = 'directory'\n\
destination = '/Docs'\n\
hidden = false # Show by default\n\
\n\
# Custom layout order stays as written.\n\
[finder]\n\
window_size = [640, 280]\n";

fn path(source: &str) -> VirtualPath {
	source.parse().unwrap()
}

fn modify(target: &str, revise: impl FnOnce(&mut ContentChanges)) -> ProjectEdit {
	let mut changes = ContentChanges::default();
	revise(&mut changes);
	ProjectEdit::Modify {
		target: path(target),
		changes,
	}
}

#[test]
fn one_batch_preserves_comments_and_restores_exact_revisions() {
	let mut editor = ProjectDocument::open(PROJECT).unwrap();
	let original = editor.revision();
	assert_eq!(editor.source(), PROJECT);
	assert!(!editor.undo().unwrap());
	assert!(
		editor
			.apply(&[
				modify("/The App.app", |edit| {
					edit.destination = Some(path("/Docs"));
					edit.position = FieldChange::Set([55.5, 30.0]);
				}),
				modify("/Docs", |edit| {
					edit.destination = Some(path("/The App.app"));
					edit.hidden = FieldChange::Set(true);
				}),
				ProjectEdit::Add(ContentDraft::Symlink {
					destination: path("/Applications"),
					target: "/Applications".to_owned(),
				}),
				ProjectEdit::SetImageName("Edition".to_owned()),
			])
			.unwrap()
	);
	let edited = editor.source().to_owned();
	let revision = editor.revision();
	assert_ne!(revision, original);
	assert!(edited.contains("'name'  = \"Edition\" # Keep the name comment"));
	assert!(edited.contains("source = 'The App.app' # Packaging source"));
	assert!(edited.contains("\t55.5, # x in points\n\t30, # y in points\n] # Icon center"));
	assert!(edited.contains("hidden = true # Show by default"));
	assert!(
		edited.contains(
			"# Custom layout order stays as written.\n[finder]\nwindow_size = [640, 280]"
		)
	);
	let parsed = Project::parse(&edited).unwrap();
	let content = &editor.project().contents;
	assert_eq!(content[0].destination, path("/Docs"));
	assert_eq!(content[1].destination, path("/The App.app"));
	assert_eq!(content[1].hidden, Some(true));
	assert_eq!(content[2].destination, path("/Applications"));
	assert_eq!(parsed.contents[0].position, content[0].position);
	assert_eq!(
		parsed.image.name.as_str(),
		editor.project().image.name.as_str()
	);
	assert!(editor.undo().unwrap());
	assert_eq!(editor.source(), PROJECT);
	assert_eq!(editor.revision(), original);
	assert_eq!(
		editor.project().contents[0].destination,
		path("/The App.app")
	);
	assert!(editor.redo().unwrap());
	assert_eq!(editor.revision(), revision);
	assert_eq!(editor.source(), edited);
}

#[test]
fn invalid_and_conflicting_edits_leave_the_current_state_and_redo_branch_intact() {
	let mut editor = ProjectDocument::open(PROJECT).unwrap();
	let original = editor.revision();
	assert!(
		editor
			.apply(&[ProjectEdit::SetImageName("Other".to_owned())])
			.unwrap()
	);
	let alternate = editor.revision();
	assert!(editor.undo().unwrap());
	let duplicate = modify("/The App.app", |edit| {
		edit.destination = Some(path("/Docs"))
	});
	assert!(matches!(
		editor.apply(&[duplicate]),
		Err(ProjectEditError::InvalidProject(
			ConfigError::Content { .. }
		))
	));
	let bad_position = modify("/The App.app", |edit| {
		edit.position = FieldChange::Set([f64::INFINITY, 0.0])
	});
	assert!(matches!(
		editor.apply(&[bad_position]),
		Err(ProjectEditError::InvalidProject(_))
	));
	assert!(matches!(
		editor.apply(&[
			ProjectEdit::Remove {
				target: path("/Docs")
			},
			modify("/Docs", |edit| edit.hidden = FieldChange::Set(true)),
		]),
		Err(ProjectEditError::ConflictingEdit { index: 1 })
	));
	assert!(matches!(
		editor.apply(&[ProjectEdit::Remove {
			target: path("/missing")
		}]),
		Err(ProjectEditError::UnknownTarget { index: 0 })
	));
	assert!(
		!editor
			.apply(&[ProjectEdit::SetImageName("Release".to_owned())])
			.unwrap()
	);
	assert_eq!(editor.source(), PROJECT);
	assert_eq!(editor.revision(), original);
	assert!(editor.redo().unwrap());
	assert_eq!(editor.revision(), alternate);
	assert!(editor.undo().unwrap());
	assert!(
		editor
			.apply(&[ProjectEdit::Remove {
				target: path("/Docs")
			}])
			.unwrap()
	);
	assert_ne!(editor.revision(), alternate);
	assert!(!editor.redo().unwrap());
	assert_eq!(editor.project().contents.len(), 1);
}

#[test]
fn inline_declarations_and_definition_references_remain_editable() {
	const SOURCE: &str = "schema_version = 1\nimage = {name = '${TITLE}', output = 'out.dmg'} # output\ncontents = [ { source = 'Original', hidden = true }, { type = 'directory', destination = '/Docs' } ] # declarations";
	let defines = BTreeMap::from([("TITLE".to_owned(), "Configured".to_owned())]);
	let mut editor = ProjectDocument::open_with_defines(SOURCE, defines).unwrap();
	assert!(
		editor
			.apply(&[
				modify("/Original", |edit| {
					edit.position = FieldChange::Set([40.0, 20.0]);
					edit.hidden = FieldChange::Clear;
				}),
				ProjectEdit::Remove {
					target: path("/Docs")
				},
				ProjectEdit::Add(ContentDraft::Copy {
					source: "Other".to_owned(),
					destination: None
				}),
			])
			.unwrap()
	);
	assert!(editor.source().starts_with(
		"schema_version = 1\nimage = {name = '${TITLE}', output = 'out.dmg'} # output\ncontents = ["
	));
	assert!(editor.source().ends_with("# declarations"));
	assert_eq!(editor.project().image.name.as_str(), "Configured");
	assert_eq!(editor.project().contents.len(), 2);
	assert_eq!(editor.project().contents[0].hidden, None);
	assert_eq!(editor.project().contents[1].destination, path("/Other"));
	assert!(editor.undo().unwrap());
	assert_eq!(editor.source(), SOURCE);
}

#[test]
fn preserves_uniform_crlf_line_endings() {
	let crlf = PROJECT.replace('\n', "\r\n");
	let mut editor = ProjectDocument::open(&crlf).unwrap();
	assert!(
		editor
			.apply(&[ProjectEdit::SetImageName("Updated".to_owned())])
			.unwrap()
	);
	assert_eq!(editor.source(), crlf.replace("'Release'", "\"Updated\""));
	assert!(editor.undo().unwrap());
	assert_eq!(editor.source(), crlf);
}

#[test]
fn opens_only_supported_and_strict_projects() {
	let future = PROJECT.replace("schema_version = 1", "schema_version = 2");
	assert!(matches!(
		ProjectDocument::open(&future),
		Err(ProjectEditError::InvalidProject(
			ConfigError::UnsupportedVersion(2)
		))
	));
	let unknown = PROJECT.replace("[image]", "[image]\nunfamiliar = true");
	assert!(matches!(
		ProjectDocument::open(&unknown),
		Err(ProjectEditError::InvalidProject(ConfigError::Schema))
	));
}
