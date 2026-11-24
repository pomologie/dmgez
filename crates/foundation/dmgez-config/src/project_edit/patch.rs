use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::{ContentChanges, ContentDraft, FieldChange, ProjectEdit, ProjectEditError};
use crate::{Project, VirtualPath};

const IMAGE: &str = "image";
const NAME: &str = "name";
const OUTPUT: &str = "output";
const CONTENTS: &str = "contents";
const DESTINATION: &str = "destination";
const POSITION: &str = "position";
const HIDDEN: &str = "hidden";
const HIDE_EXTENSION: &str = "hide_extension";
const KIND: &str = "type";
const SOURCE: &str = "source";
const TARGET: &str = "target";
const DIRECTORY: &str = "directory";
const SYMLINK: &str = "symlink";

pub(super) fn apply(
	document: &mut DocumentMut,
	project: &Project,
	edits: &[ProjectEdit],
) -> Result<(), ProjectEditError> {
	let mut selected = BTreeMap::new();
	for (index, entry) in project.contents.iter().enumerate() {
		selected.insert(&entry.destination, index);
	}
	let mut modified = BTreeSet::new();
	let mut removed = BTreeSet::new();
	for (index, edit) in edits.iter().enumerate() {
		let select = |target: &VirtualPath| {
			selected
				.get(target)
				.copied()
				.ok_or(ProjectEditError::UnknownTarget { index })
		};
		match edit {
			ProjectEdit::Add(draft) => {
				if !document.contains_key(CONTENTS) {
					document.insert(CONTENTS, Item::ArrayOfTables(ArrayOfTables::new()));
				}
				contents(document)?.append(draft);
			}
			ProjectEdit::Modify { target, changes } => {
				let slot = select(target)?;
				if removed.contains(&slot) {
					return Err(ProjectEditError::ConflictingEdit { index });
				}
				modified.insert(slot);
				modify(contents(document)?.entry(slot)?, changes);
			}
			ProjectEdit::Remove { target } => {
				let slot = select(target)?;
				if modified.contains(&slot) || !removed.insert(slot) {
					return Err(ProjectEditError::ConflictingEdit { index });
				}
			}
			ProjectEdit::SetImageName(name) => set_string(image(document)?, NAME, name),
			ProjectEdit::SetOutputPath(path) => set_string(image(document)?, OUTPUT, path),
		}
	}
	if !removed.is_empty() {
		let mut entries = contents(document)?;
		for index in removed.into_iter().rev() {
			entries.remove(index)?;
		}
	}
	Ok(())
}

fn image(document: &mut DocumentMut) -> Result<&mut dyn TableLike, ProjectEditError> {
	document
		.get_mut(IMAGE)
		.and_then(Item::as_table_like_mut)
		.ok_or(ProjectEditError::Structure)
}

fn contents(document: &mut DocumentMut) -> Result<Contents<'_>, ProjectEditError> {
	match document.get_mut(CONTENTS) {
		Some(Item::ArrayOfTables(tables)) => Ok(Contents::Tables(tables)),
		Some(Item::Value(Value::Array(array))) => Ok(Contents::Inline(array)),
		_ => Err(ProjectEditError::Structure),
	}
}

enum Contents<'document> {
	Tables(&'document mut ArrayOfTables),
	Inline(&'document mut Array),
}

impl Contents<'_> {
	fn entry(&mut self, index: usize) -> Result<&mut dyn TableLike, ProjectEditError> {
		match self {
			Self::Tables(tables) => tables
				.get_mut(index)
				.map(|table| table as &mut dyn TableLike),
			Self::Inline(array) => array
				.get_mut(index)
				.and_then(Value::as_inline_table_mut)
				.map(|table| table as &mut dyn TableLike),
		}
		.ok_or(ProjectEditError::Structure)
	}

	fn append(&mut self, draft: &ContentDraft) {
		match self {
			Self::Tables(tables) => {
				let mut table = Table::new();
				fill_entry(&mut table, draft);
				tables.push(table);
			}
			Self::Inline(array) => {
				let mut table = InlineTable::new();
				fill_entry(&mut table, draft);
				array.push(table);
			}
		}
	}

	fn remove(&mut self, index: usize) -> Result<(), ProjectEditError> {
		self.entry(index)?;
		match self {
			Self::Tables(tables) => {
				tables.remove(index);
			}
			Self::Inline(array) => {
				array.remove(index);
			}
		}
		Ok(())
	}
}

fn fill_entry(table: &mut dyn TableLike, draft: &ContentDraft) {
	match draft {
		ContentDraft::Copy {
			source,
			destination,
		} => {
			set_string(table, SOURCE, source);
			if let Some(destination) = destination {
				set_string(table, DESTINATION, destination.as_str());
			}
		}
		ContentDraft::Directory { destination } => {
			set_string(table, KIND, DIRECTORY);
			set_string(table, DESTINATION, destination.as_str());
		}
		ContentDraft::Symlink {
			destination,
			target,
		} => {
			set_string(table, KIND, SYMLINK);
			set_string(table, DESTINATION, destination.as_str());
			set_string(table, TARGET, target);
		}
	}
}

fn modify(table: &mut dyn TableLike, changes: &ContentChanges) {
	if let Some(destination) = &changes.destination {
		set_string(table, DESTINATION, destination.as_str());
	}
	match changes.position {
		FieldChange::Keep => {}
		FieldChange::Set(position) => set_position(table, position),
		FieldChange::Clear => {
			table.remove(POSITION);
		}
	}
	apply_flag(table, HIDDEN, changes.hidden);
	apply_flag(table, HIDE_EXTENSION, changes.hide_extension);
}

fn set_position(table: &mut dyn TableLike, position: [f64; 2]) {
	if let Some(array) = table.get_mut(POSITION).and_then(Item::as_array_mut) {
		for (previous, next) in array.iter_mut().zip(position) {
			let previous_number = previous
				.as_float()
				.or_else(|| previous.as_integer().map(|i| i as f64));
			if previous_number != Some(next) {
				replace_value(previous, next.into());
			}
		}
	} else {
		let array: Array = position.into_iter().collect();
		table.insert(POSITION, Item::Value(array.into()));
	}
}

fn set_string(table: &mut dyn TableLike, key: &str, value: &str) {
	if let Some(previous) = table.get_mut(key).and_then(Item::as_value_mut) {
		if previous.as_str() != Some(value) {
			replace_value(previous, value.into());
		}
	} else {
		table.insert(key, Item::Value(value.into()));
	}
}

fn apply_flag(table: &mut dyn TableLike, key: &str, change: FieldChange<bool>) {
	match change {
		FieldChange::Keep => {}
		FieldChange::Set(flag) => {
			if let Some(previous) = table.get_mut(key).and_then(Item::as_value_mut) {
				if previous.as_bool() != Some(flag) {
					replace_value(previous, flag.into());
				}
			} else {
				table.insert(key, Item::Value(flag.into()));
			}
		}
		FieldChange::Clear => {
			table.remove(key);
		}
	}
}

fn replace_value(previous: &mut Value, mut next: Value) {
	*next.decor_mut() = previous.decor().clone();
	*previous = next;
}
