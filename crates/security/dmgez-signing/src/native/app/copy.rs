//! Checked, metadata-preserving copy into the private native signing stage.

use std::{
	ffi::OsStr,
	fs, io,
	path::{Path, PathBuf},
	process::ExitStatus,
};

use crate::native::runner;

mod contents;
mod inventory;

#[cfg(test)]
mod tests;

use inventory::{Inspection, Inventory};

// Keep native resource, attribute, ACL and quarantine copy semantics explicit.
const COPY_OPTIONS: [&str; 6] = ["--rsrc", "--extattr", "--acl", "--qtn", "-X", "--"];

#[derive(Debug, thiserror::Error)]
pub(super) enum CopyError {
	#[error("the source and stage must be distinct absolute bundle paths")]
	Location,
	#[error("the stage bundle already exists")]
	Exists,
	#[error("could not inspect bundle entry {path:?}")]
	Io {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("unsupported filesystem entry in the bundle at {0:?}")]
	Unsupported(PathBuf),
	#[error("bundle link {0:?} does not resolve within the bundle")]
	Link(PathBuf),
	#[error("the app bundle exceeds the inspection limit for {0}")]
	Limit(&'static str),
	#[error("the app source changed during inspection at {0:?}")]
	SourceChanged(PathBuf),
	#[error("the staged copy differs from the source at {0:?}")]
	CopyDiffers(PathBuf),
	#[error("could not run the native app copy tool")]
	Tool(#[source] runner::Error),
	#[error("the native app copy tool failed ({0})")]
	Failed(ExitStatus),
}

pub(super) struct Snapshot {
	source: PathBuf,
	inventory: Inventory,
}

/// Copies a stable source tree into an absent child of a private stage.
///
/// The caller holds the stage and validates its directory anchors before using
/// this pathname-based native tool. The source's canonical path and tree must
/// remain stable while preparing and signing the copy.
pub(super) fn copy_bundle(source: &Path, destination: &Path) -> Result<Snapshot, CopyError> {
	if !source.is_absolute()
		|| !destination.is_absolute()
		|| destination.starts_with(source)
		|| source.starts_with(destination)
	{
		return Err(CopyError::Location);
	}
	let snapshot = Snapshot::capture(source, Inspection::Full)?;
	match fs::symlink_metadata(destination) {
		Err(error) if error.kind() == io::ErrorKind::NotFound => {}
		Err(error) => return Err(at(destination, error)),
		Ok(_) => return Err(CopyError::Exists),
	}
	let mut args: Vec<&OsStr> = COPY_OPTIONS.iter().map(OsStr::new).collect();
	args.extend([source.as_os_str(), destination.as_os_str()]);
	let output = runner::ditto(&args).map_err(CopyError::Tool)?;
	if !output.status.success() {
		return Err(CopyError::Failed(output.status));
	}
	snapshot.validate_source()?;
	let staged = Inventory::read(destination, Inspection::Full)?;
	snapshot.inventory.compare_copy(&staged)?;
	Ok(snapshot)
}

impl Snapshot {
	pub(super) fn observe(source: &Path) -> Result<Self, CopyError> {
		Self::capture(source, Inspection::Stamps)
	}

	fn capture(source: &Path, inspection: Inspection) -> Result<Self, CopyError> {
		Ok(Self {
			source: source.to_path_buf(),
			inventory: Inventory::read(source, inspection)?,
		})
	}

	pub(super) fn validate_source(&self) -> Result<(), CopyError> {
		let current = Inventory::read(&self.source, Inspection::Stamps)?;
		self.inventory.compare_source(&current)
	}
}

fn at(path: &Path, source: impl Into<io::Error>) -> CopyError {
	CopyError::Io {
		path: path.to_path_buf(),
		source: source.into(),
	}
}
