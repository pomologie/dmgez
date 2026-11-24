use std::{
	fs::{self, File},
	io::{self, Read},
	path::{Component, Path, PathBuf},
};

use dmgez_config::{
	MAX_MANIFEST_BYTES,
	project_edit::{ProjectDocument, ProjectEditError},
};
use dmgez_core::output::{PublishError, publish_new};
use dmgez_inspector::{DiskLimits, InspectionReport, RunWindow, inspect_disk};
use dmgez_udif::reader::{FileSource, Image, Limits};
use thiserror::Error;

use crate::project::ProjectTab;

pub(crate) const PROJECT_EXTENSION: &str = "toml";
pub(crate) const IMAGE_EXTENSION: &str = "dmg";
const FIRST_RUN: usize = 0;
const RUN_PAGE_SIZE: usize = 64;

pub(crate) struct Opened {
	pub path: PathBuf,
	pub document: Document,
}

pub(crate) enum Document {
	Project(Box<ProjectTab>),
	Image(Box<InspectionReport>),
}

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error("choose a TOML project or a DMG image")]
	Extension,
	#[error("choose a regular file")]
	NotFile,
	#[error("project exceeds the supported manifest size")]
	ProjectLimit,
	#[error("enter a .toml filename for the project copy")]
	CopyName,
	#[error("invalid project: {0}")]
	Project(#[from] ProjectEditError),
	#[error("could not open image: {0}")]
	Udif(#[from] dmgez_udif::Error),
	#[error("could not open or read the selected file: {0}")]
	Io(#[from] io::Error),
	#[error("could not save the project copy: {0}")]
	Save(#[from] PublishError<io::Error>),
}

pub(crate) fn open(path: PathBuf) -> Result<Opened, Error> {
	let extension = path
		.extension()
		.and_then(|extension| extension.to_str())
		.ok_or(Error::Extension)?;
	let is_project = extension.eq_ignore_ascii_case(PROJECT_EXTENSION);
	if !is_project && !extension.eq_ignore_ascii_case(IMAGE_EXTENSION) {
		return Err(Error::Extension);
	}
	let canonical_path = path.canonicalize()?;
	if !fs::metadata(&canonical_path)?.is_file() {
		return Err(Error::NotFile);
	}
	let input = File::open(&canonical_path)?;
	if !input.metadata()?.is_file() {
		return Err(Error::NotFile);
	}
	let document = if is_project {
		let document = read_project(input)?;
		Document::Project(Box::new(ProjectTab::new(document)))
	} else {
		let mut image = Image::open(FileSource::new(input)?, Limits::default())?;
		let report = inspect_disk(
			&mut image,
			RunWindow::new(FIRST_RUN, RUN_PAGE_SIZE),
			DiskLimits::default(),
		);
		Document::Image(Box::new(report))
	};
	Ok(Opened {
		path: canonical_path,
		document,
	})
}

fn read_project(input: impl Read) -> Result<ProjectDocument, Error> {
	let mut source = String::new();
	input
		.take(MAX_MANIFEST_BYTES as u64 + 1)
		.read_to_string(&mut source)?;
	if source.len() > MAX_MANIFEST_BYTES {
		return Err(Error::ProjectLimit);
	}
	Ok(ProjectDocument::open(&source)?)
}

pub(crate) fn save_copy(path: &Path, filename: &str, source: &str) -> Result<PathBuf, Error> {
	let filename = Path::new(filename);
	let mut components = filename.components();
	if !matches!(components.next(), Some(Component::Normal(_)))
		|| components.next().is_some()
		|| filename
			.extension()
			.and_then(|extension| extension.to_str())
			.is_none_or(|extension| !extension.eq_ignore_ascii_case(PROJECT_EXTENSION))
	{
		return Err(Error::CopyName);
	}
	let parent = path.parent().ok_or(Error::CopyName)?;
	let destination = parent.join(filename);
	publish_new(
		&destination,
		|file| file.write_all(source.as_bytes()),
		|_, _| Ok::<_, io::Error>(()),
	)?;
	Ok(destination)
}

#[cfg(test)]
mod tests {
	use super::*;

	const MANIFEST: &str = "# preserved comment\nschema_version = 1\n[image]\nname = 'Example'\noutput = 'dist/example.dmg'\n";
	const PROJECT_NAME: &str = "example.toml";
	const COPY_NAME: &str = "copy.toml";

	#[test]
	fn opens_typed_project_and_writes_a_fresh_copy_beside_it() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join(PROJECT_NAME);
		fs::write(&path, MANIFEST).unwrap();
		let opened = open(path.clone()).unwrap();
		let Document::Project(project) = opened.document else {
			panic!("expected project")
		};
		assert_eq!(project.document.source(), MANIFEST);
		let copy = save_copy(&path, COPY_NAME, project.document.source()).unwrap();
		assert_eq!(copy.parent(), path.parent());
		assert_eq!(fs::read_to_string(&copy).unwrap(), MANIFEST);
		assert!(matches!(
			save_copy(&path, COPY_NAME, "conflict"),
			Err(Error::Save(_))
		));
		assert!(matches!(
			save_copy(&path, "../escape.toml", MANIFEST),
			Err(Error::CopyName)
		));
		assert_eq!(fs::read_to_string(&copy).unwrap(), MANIFEST);
		assert_eq!(fs::read_to_string(&path).unwrap(), MANIFEST);
	}

	#[test]
	fn bounds_project_read_and_rejects_unrecognized_files() {
		let over_limit = io::repeat(b' ').take(MAX_MANIFEST_BYTES as u64 + 1);
		assert!(matches!(read_project(over_limit), Err(Error::ProjectLimit)));
		assert!(matches!(
			open(PathBuf::from("never.exe")),
			Err(Error::Extension)
		));
	}

	#[test]
	fn opens_the_native_gpt_fixture_as_an_actual_disk_report() {
		use dmgez_inspector::{DiskReport, DiskScheme};

		const FIXTURE: &[u8] =
			include_bytes!("../../../crates/foundation/dmgez-vfs/tests/fixtures/native-gpt.dmg");
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("example.dmg");
		fs::write(&path, FIXTURE).unwrap();
		let opened = open(path).unwrap();
		let Document::Image(report) = opened.document else {
			panic!("expected an image inspection report");
		};
		assert!(matches!(
			report.disk,
			Some(DiskReport::Partitioned { scheme: DiskScheme::Gpt, ref partitions, .. })
				if !partitions.is_empty()
		));
		assert!(report.run_page.is_some());
	}
}
