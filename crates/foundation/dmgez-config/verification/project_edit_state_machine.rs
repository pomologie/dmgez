//! Production-linked edit history checks for D-V11.

use std::{collections::HashSet, path::Path};

use dmgez_config::{
	ConfigError, ContentError, Coordinate, Payload, Project, VirtualPath,
	project_edit::{
		ContentChanges, ContentDraft, FieldChange, ProjectDocument, ProjectEdit, ProjectEditError,
		RevisionId,
	},
};
use proptest::{
	prelude::*,
	test_runner::{TestCaseError, TestCaseResult},
};

const MAX_ACTIONS: usize = 20;
const DESTINATIONS: [&str; 4] = ["/App.app", "/Docs", "/Applications", "/Notes"];
const SOURCE_APP: &str = "payload/App.app";
const LINK_TARGET: &str = "/Applications";
const ABSENT: &str = "/Absent";
const TITLE: &str = "Installer";
const OUTPUT: &str = "Installer.dmg";
const TITLES: &[&str] = &[TITLE, "Preview"];
const OUTPUTS: &[&str] = &[OUTPUT, "preview.dmg"];
const POSITIONS: &[[f64; 2]] = &[[16.0, 32.0], [40.25, 30.5], [-8.0, 0.0]];
const KINDS: &[Kind] = &[Kind::Copy, Kind::Directory, Kind::Symlink];
const COMMENTS: [&str; 3] = ["# packaging", "# volume label", "# image destination"];

const INLINE: &str = "\
# packaging
schema_version = 1
contents = [
	{source = 'payload/App.app', position = [16, 32], hidden = false},
	{type = 'directory', destination = '/Docs', hide_extension = true},
]

[image]
name   = 'Installer' # volume label
output = 'Installer.dmg' # image destination
";

const TABLES: &str = "\
# packaging
schema_version = 1

[image]
name   = 'Installer' # volume label
output = 'Installer.dmg' # image destination

[[contents]]
source = 'payload/App.app'
position = [16, 32]
hidden = false

[[contents]]
type = 'directory'
destination = '/Docs'
hide_extension = true
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
	Copy,
	Directory,
	Symlink,
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
	slot: usize,
	kind: Kind,
	hidden: Option<bool>,
	hide_extension: Option<bool>,
	position: Option<[f64; 2]>,
}

impl Entry {
	fn new(slot: usize, kind: Kind) -> Self {
		Self {
			slot,
			kind,
			hidden: None,
			hide_extension: None,
			position: None,
		}
	}
}

#[derive(Clone, Debug, PartialEq)]
struct Model {
	title: &'static str,
	output: &'static str,
	contents: Vec<Entry>,
}

impl Model {
	fn initial() -> Self {
		let mut app = Entry::new(0, Kind::Copy);
		app.hidden = Some(false);
		app.position = Some(POSITIONS[0]);
		let mut docs = Entry::new(1, Kind::Directory);
		docs.hide_extension = Some(true);
		Self {
			title: TITLE,
			output: OUTPUT,
			contents: vec![app, docs],
		}
	}

	fn index(&self, slot: usize, command_index: usize) -> Result<usize, Rejection> {
		self.contents
			.iter()
			.position(|entry| entry.slot == slot)
			.ok_or(Rejection::UnknownTarget(command_index))
	}

	fn edited(&self, edit: Edit) -> Result<Self, Rejection> {
		let mut next = self.clone();
		match edit {
			Edit::Add(slot, kind) => {
				if self.contents.iter().any(|entry| entry.slot == slot) {
					return Err(Rejection::Duplicate);
				}
				next.contents.push(Entry::new(slot, kind));
			}
			Edit::Remove(slot) => {
				next.contents.remove(self.index(slot, 0)?);
			}
			Edit::Modify(slot, patch) => {
				let entry = &mut next.contents[self.index(slot, 0)?];
				if let Some(to) = patch.destination {
					if to != slot && self.contents.iter().any(|other| other.slot == to) {
						return Err(Rejection::Duplicate);
					}
					entry.slot = to;
				}
				entry.hidden = changed(entry.hidden, patch.hidden);
				entry.hide_extension = changed(entry.hide_extension, patch.hide_extension);
				entry.position = changed(entry.position, patch.position);
			}
			Edit::Swap(first, second) => {
				next.contents[self.index(first, 0)?].slot = second;
				next.contents[self.index(second, 1)?].slot = first;
			}
			Edit::Title(title) => next.title = title,
			Edit::Output(output) => next.output = output,
			Edit::LateRejection => return Err(Rejection::UnknownTarget(1)),
			Edit::Empty => {}
		}
		Ok(next)
	}
}

fn changed<T>(previous: Option<T>, field: FieldChange<T>) -> Option<T> {
	match field {
		FieldChange::Keep => previous,
		FieldChange::Set(value) => Some(value),
		FieldChange::Clear => None,
	}
}

#[derive(Clone, Copy, Debug)]
struct Patch {
	destination: Option<usize>,
	hidden: FieldChange<bool>,
	hide_extension: FieldChange<bool>,
	position: FieldChange<[f64; 2]>,
}

#[derive(Clone, Copy, Debug)]
enum Edit {
	Add(usize, Kind),
	Remove(usize),
	Modify(usize, Patch),
	Swap(usize, usize),
	Title(&'static str),
	Output(&'static str),
	LateRejection,
	Empty,
}

impl Edit {
	fn commands(self) -> Vec<ProjectEdit> {
		match self {
			Edit::Add(slot, kind) => {
				let destination = path(slot);
				let draft = match kind {
					Kind::Copy => ContentDraft::Copy {
						source: SOURCE_APP.into(),
						destination: Some(destination),
					},
					Kind::Directory => ContentDraft::Directory { destination },
					Kind::Symlink => ContentDraft::Symlink {
						destination,
						target: LINK_TARGET.into(),
					},
				};
				vec![ProjectEdit::Add(draft)]
			}
			Edit::Remove(slot) => vec![ProjectEdit::Remove { target: path(slot) }],
			Edit::Modify(slot, patch) => {
				let mut changes = ContentChanges::default();
				changes.destination = patch.destination.filter(|&to| to != slot).map(path);
				changes.hidden = patch.hidden;
				changes.hide_extension = patch.hide_extension;
				changes.position = patch.position;
				vec![ProjectEdit::Modify {
					target: path(slot),
					changes,
				}]
			}
			Edit::Swap(first, second) => vec![rename(first, second), rename(second, first)],
			Edit::Title(title) => vec![ProjectEdit::SetImageName(title.into())],
			Edit::Output(output) => vec![ProjectEdit::SetOutputPath(output.into())],
			Edit::LateRejection => vec![
				ProjectEdit::SetImageName(TITLES[1].into()),
				ProjectEdit::Remove {
					target: ABSENT.parse().unwrap(),
				},
			],
			Edit::Empty => Vec::new(),
		}
	}
}

fn path(slot: usize) -> VirtualPath {
	DESTINATIONS[slot].parse().unwrap()
}

fn rename(slot: usize, destination: usize) -> ProjectEdit {
	let mut changes = ContentChanges::default();
	changes.destination = Some(path(destination));
	ProjectEdit::Modify {
		target: path(slot),
		changes,
	}
}

#[derive(Debug, PartialEq, Eq)]
enum Rejection {
	UnknownTarget(usize),
	Duplicate,
	Unexpected(ProjectEditError),
}

impl From<ProjectEditError> for Rejection {
	fn from(error: ProjectEditError) -> Self {
		match error {
			ProjectEditError::UnknownTarget { index } => Self::UnknownTarget(index),
			ProjectEditError::InvalidProject(ConfigError::Content {
				error: ContentError::DuplicateDestination { .. },
				..
			}) => Self::Duplicate,
			other => Self::Unexpected(other),
		}
	}
}

#[derive(Clone, Copy, Debug)]
enum Action {
	Apply(Edit),
	Undo,
	Redo,
}

fn slot() -> impl Strategy<Value = usize> {
	0..DESTINATIONS.len()
}

fn field<T: Clone + std::fmt::Debug + 'static>(
	values: impl Strategy<Value = T> + 'static,
) -> impl Strategy<Value = FieldChange<T>> {
	prop_oneof![
		Just(FieldChange::Keep),
		Just(FieldChange::Clear),
		values.prop_map(FieldChange::Set),
	]
}

fn action() -> impl Strategy<Value = Action> {
	let edits = prop_oneof![
		2 => (slot(), prop::sample::select(KINDS))
			.prop_map(|(slot, kind)| Edit::Add(slot, kind)),
		2 => slot().prop_map(Edit::Remove),
		4 => (slot(), prop::option::of(slot()), field(any::<bool>()), field(any::<bool>()),
			field(prop::sample::select(POSITIONS)))
			.prop_map(|(slot, destination, hidden, hide_extension, position)| Edit::Modify(slot,
				Patch { destination, hidden, hide_extension, position })),
		2 => (slot(), 1..DESTINATIONS.len())
			.prop_map(|(first, gap)| Edit::Swap(first, (first + gap) % DESTINATIONS.len())),
		1 => prop::sample::select(TITLES).prop_map(Edit::Title),
		1 => prop::sample::select(OUTPUTS).prop_map(Edit::Output),
		1 => Just(Edit::LateRejection),
		1 => Just(Edit::Empty),
	];
	prop_oneof![
		7 => edits.prop_map(Action::Apply),
		2 => Just(Action::Undo),
		2 => Just(Action::Redo),
	]
}

struct Revision {
	model: Model,
	source: String,
	id: RevisionId,
}

impl Revision {
	fn observe(editor: &ProjectDocument, model: Model) -> Self {
		Self {
			model,
			source: editor.source().to_owned(),
			id: editor.revision(),
		}
	}
}

fn check_semantics(project: &Project, expected: &Model) -> TestCaseResult {
	prop_assert_eq!(project.image.name.as_str(), expected.title);
	prop_assert_eq!(project.image.output.as_path(), Path::new(expected.output));
	prop_assert_eq!(project.contents.len(), expected.contents.len());
	for (actual, expected) in project.contents.iter().zip(&expected.contents) {
		prop_assert_eq!(actual.destination.as_str(), DESTINATIONS[expected.slot]);
		prop_assert_eq!(actual.hidden, expected.hidden);
		prop_assert_eq!(actual.hide_extension, expected.hide_extension);
		prop_assert_eq!(
			actual.position.map(|point| point.map(Coordinate::get)),
			expected.position
		);
		match (&actual.payload, expected.kind) {
			(Payload::Copy(source), Kind::Copy) => {
				prop_assert_eq!(source.source.as_path(), Path::new(SOURCE_APP));
			}
			(Payload::Symlink { target }, Kind::Symlink) => {
				prop_assert_eq!(target.as_str(), LINK_TARGET);
			}
			(Payload::Directory, Kind::Directory) => {}
			(other, kind) => prop_assert!(false, "expected {kind:?}, got {other:?}"),
		}
	}
	Ok(())
}

proptest! {
	#[test]
	fn edit_history_matches_a_bounded_reference_model(
		actions in prop::collection::vec(action(), 0..=MAX_ACTIONS),
		inline: bool, crlf: bool, final_newline: bool,
	) {
		let mut source = if inline { INLINE } else { TABLES }.to_owned();
		if !final_newline {
			source.pop();
		}
		if crlf {
			source = source.replace('\n', "\r\n");
		}
		let mut editor = ProjectDocument::open(&source).unwrap();
		let mut timeline = vec![Revision::observe(&editor, Model::initial())];
		let mut cursor = 0;
		let mut allocated = HashSet::from([editor.revision()]);
		check_semantics(editor.project(), &timeline[cursor].model)?;
		prop_assert_eq!(editor.source(), &source);

		for action in actions {
			match action {
				Action::Apply(edit) => {
					let current = &timeline[cursor].model;
					let observed = editor.apply(&edit.commands()).map_err(Rejection::from);
					match current.edited(edit) {
						Ok(next) if &next != current => {
							prop_assert_eq!(observed, Ok(true), "applying {:?}", edit);
							prop_assert_ne!(editor.source(), timeline[cursor].source.as_str());
							prop_assert!(allocated.insert(editor.revision()), "revision ID reused");
							timeline.truncate(cursor + 1);
							timeline.push(Revision::observe(&editor, next));
							cursor += 1;
						}
						Ok(_) => prop_assert_eq!(observed, Ok(false), "applying {:?}", edit),
						Err(error) => prop_assert_eq!(observed, Err(error), "applying {:?}", edit),
					}
				}
				Action::Undo => {
					let previous = cursor.checked_sub(1);
					prop_assert_eq!(editor.undo(), Ok(previous.is_some()));
					cursor = previous.unwrap_or(cursor);
				}
				Action::Redo => {
					let has_next = cursor + 1 < timeline.len();
					prop_assert_eq!(editor.redo(), Ok(has_next));
					cursor += usize::from(has_next);
				}
			}

			let expected = &timeline[cursor];
			prop_assert_eq!(editor.revision(), expected.id);
			prop_assert_eq!(editor.source(), &expected.source);
			prop_assert_eq!(editor.source().ends_with('\n'), final_newline);
			prop_assert_eq!(
				editor.source().matches("\r\n").count(),
				if crlf { editor.source().matches('\n').count() } else { 0 },
			);
			for comment in COMMENTS {
				prop_assert_eq!(editor.source().matches(comment).count(), 1);
			}
			check_semantics(editor.project(), &expected.model)?;
			let reparsed = Project::parse(editor.source())
				.map_err(|error| TestCaseError::fail(error.to_string()))?;
			check_semantics(&reparsed, &expected.model)?;
		}
	}
}
