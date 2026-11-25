use std::{
	fs, io,
	path::{Path, PathBuf},
};

use dmgez_config::{
	VirtualPath, VirtualPathError,
	project_edit::{ContentDraft, ProjectEdit},
};
use thiserror::Error;

const MAX_ENTRIES: usize = 256;
const IMAGE_SEPARATOR: char = '/';
const DEFINE_START: &str = "${";

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error("select at most {MAX_ENTRIES} entries at a time")]
	EntryCount,
	#[error("a selected path has no absolute file location")]
	Location,
	#[error("selected paths must be representable as UTF-8 in a project")]
	NonUnicode,
	#[error("selected paths cannot contain the project variable marker {DEFINE_START:?}")]
	VariableSyntax,
	#[error("the selection includes an entry other than a file, directory, or symlink: {0:?}")]
	UnsupportedKind(PathBuf),
	#[error("could not inspect selected path {path:?}: {source}")]
	Inspect { path: PathBuf, source: io::Error },
	#[error("the selected name cannot be used as an image path: {path:?}")]
	Destination {
		path: PathBuf,
		source: VirtualPathError,
	},
}

pub(crate) fn pick_files(directory: &Path) -> Option<Vec<PathBuf>> {
	rfd::FileDialog::new().set_directory(directory).pick_files()
}

/// Prepares one batch from a picker selection or native drop. The worker reads
/// metadata only and preserves symlink leaves selected as project content.
pub(crate) fn prepare(
	project_directory: &Path,
	folder: &VirtualPath,
	selected: impl IntoIterator<Item: AsRef<Path>, IntoIter: ExactSizeIterator>,
) -> Result<Vec<ProjectEdit>, Error> {
	let selected = selected.into_iter();
	if selected.len() > MAX_ENTRIES {
		return Err(Error::EntryCount);
	}
	let folder = if folder.is_root() {
		""
	} else {
		folder.as_str()
	};
	selected
		.map(|path| {
			let path = path.as_ref();
			let name = path.file_name().ok_or(Error::Location)?;
			let parent = path
				.parent()
				.filter(|_| path.is_absolute())
				.ok_or(Error::Location)?;
			let name_text = name.to_str().ok_or(Error::NonUnicode)?;
			let destination = format!("{folder}{IMAGE_SEPARATOR}{name_text}");
			let inspect = |source| Error::Inspect {
				path: path.to_path_buf(),
				source,
			};
			let source_path = parent.canonicalize().map_err(inspect)?.join(name);
			let kind = fs::symlink_metadata(&source_path)
				.map_err(inspect)?
				.file_type();
			if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
				return Err(Error::UnsupportedKind(path.to_path_buf()));
			}
			let source = source_path
				.strip_prefix(project_directory)
				.unwrap_or(&source_path);
			let source = source.to_str().ok_or(Error::NonUnicode)?;
			if source.contains(DEFINE_START) || destination.contains(DEFINE_START) {
				return Err(Error::VariableSyntax);
			}
			let destination = destination.parse().map_err(|source| Error::Destination {
				path: path.to_path_buf(),
				source,
			})?;
			Ok(ProjectEdit::Add(ContentDraft::Copy {
				source: source.to_owned(),
				destination: Some(destination),
			}))
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use dmgez_config::project_edit::ProjectDocument;

	const PROJECT: &str = "# kept\nschema_version=1\n[image]\nname='Import'\noutput='import.dmg'\n";

	#[test]
	fn prepares_paths_against_the_project_base_and_virtual_folder() {
		let work = tempfile::tempdir().unwrap();
		let work = work.path().canonicalize().unwrap();
		let project = work.join("project");
		fs::create_dir(&project).unwrap();
		let internal = project.join("Read Me.txt");
		let external = work.join("outside.txt");
		for path in [&internal, &external] {
			fs::write(path, b"input").unwrap();
		}
		let edits = prepare(
			&project,
			&"/Docs".parse().unwrap(),
			vec![internal, external.clone()],
		)
		.unwrap();
		let mut document = ProjectDocument::open(PROJECT).unwrap();
		document.apply(&edits).unwrap();
		let [first, second] = document.project().contents.as_slice() else {
			panic!("expected two declarations")
		};
		assert_eq!(first.destination.as_str(), "/Docs/Read Me.txt");
		assert_eq!(second.destination.as_str(), "/Docs/outside.txt");
		let dmgez_config::Payload::Copy(first) = &first.payload else {
			panic!("expected copy")
		};
		let dmgez_config::Payload::Copy(second) = &second.payload else {
			panic!("expected copy")
		};
		assert_eq!(first.source.as_path(), Path::new("Read Me.txt"));
		assert_eq!(second.source.as_path(), external);
		assert!(document.source().starts_with("# kept"));
	}

	#[test]
	fn rejects_over_budget_and_unrepresentable_paths() {
		let scratch = tempfile::tempdir().unwrap();
		let base = scratch.path().canonicalize().unwrap();
		let root = "/".parse().unwrap();
		assert!(matches!(
			prepare(&base, &root, vec![base.join("unused"); MAX_ENTRIES + 1]),
			Err(Error::EntryCount)
		));
		assert!(matches!(
			prepare(&base, &root, [base.ancestors().last().unwrap()]),
			Err(Error::Location)
		));
		let special = base.join("file-${NAME}.txt");
		fs::write(&special, b"input").unwrap();
		assert!(matches!(
			prepare(&base, &root, vec![special]),
			Err(Error::VariableSyntax)
		));
	}

	#[cfg(unix)]
	#[test]
	fn retains_the_selected_symlink_even_when_its_target_is_absent() {
		use std::os::unix::fs::symlink;

		let scratch = tempfile::tempdir().unwrap();
		let base = scratch.path().canonicalize().unwrap();
		let link = base.join("shortcut");
		symlink("absent-target", &link).unwrap();
		let edits = prepare(&base, &"/".parse().unwrap(), vec![link]).unwrap();
		assert!(matches!(
			edits.as_slice(),
			[ProjectEdit::Add(ContentDraft::Copy { source, .. })] if source == "shortcut"
		));
	}
}
