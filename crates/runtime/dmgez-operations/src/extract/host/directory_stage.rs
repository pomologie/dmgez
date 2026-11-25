use std::{io, path::PathBuf};

use rustix::{
	fs::{self, AtFlags, FileType, Mode, RenameFlags},
	io::Errno,
};

use super::{
	DestinationDirectory, HostError, HostName,
	directory::{PRIVATE_DIRECTORY, identity, inspect, stat_entry},
	stage::with_stage_name,
};
use crate::extract::{DEFAULT_COMPONENT_BYTES, NameProfile};

/// A bundle directory held inside an owner-only staging directory.
///
/// The caller must maintain a stable destination namespace and restrict access
/// to the stage, including any inherited ACLs. On drop only empty, still
/// attached directories are removed. Use [`Self::cleanup`] to report residue.
#[derive(Debug)]
pub(crate) struct DirectoryStage<'parent, 'root> {
	parent: &'parent DestinationDirectory<'root>,
	private: DestinationDirectory<'root>,
	directory: DestinationDirectory<'root>,
	name: HostName,
	target: HostName,
	state: State,
}

#[derive(Debug)]
enum State {
	Staging,
	EmptyWrapper,
	Renamed,
	Cleaned { published: bool },
}

impl<'root> DestinationDirectory<'root> {
	/// Reserve a private stage under this destination for one fresh directory.
	///
	/// The directory to publish is itself a child of the owner-only wrapper,
	/// allowing the caller to set final permissions before publication. The
	/// private wrapper uses the ordinary two-level destination depth budget.
	pub(crate) fn stage_directory(
		&self,
		target: &HostName,
	) -> Result<DirectoryStage<'_, 'root>, HostError> {
		self.require_absent(target)?;
		let (name, private) = with_stage_name(self.root_path(), target, |candidate| {
			let candidate = candidate.to_str().ok_or(io::ErrorKind::InvalidInput)?;
			let name = HostName::new(
				candidate,
				NameProfile::PortableAscii,
				DEFAULT_COMPONENT_BYTES,
			)
			.map_err(io::Error::other)?;
			match self.create_child(&name) {
				Err(HostError::Conflict) => Err(io::ErrorKind::AlreadyExists.into()),
				result => Ok((name, result)),
			}
		})
		.map_err(HostError::CreateDirectoryStage)?;
		let private = match private {
			Ok(private) => private,
			Err(source @ HostError::CreatedDirectoryUnavailable { .. }) => {
				return Err(unavailable(name, source));
			}
			Err(error) => return Err(error),
		};
		let prepared = owner_only(&private)
			.and_then(|()| self.require_absent(target))
			.and_then(|()| private.create_child(target));
		let directory = match prepared {
			Ok(directory) => directory,
			Err(source) => {
				return Err(if remove_empty(self, &name, &private).is_ok() {
					source
				} else {
					unavailable(name, source)
				});
			}
		};
		Ok(DirectoryStage {
			parent: self,
			private,
			directory,
			name,
			target: target.clone(),
			state: State::Staging,
		})
	}
}

impl<'root> DirectoryStage<'_, 'root> {
	/// The descriptor-scoped directory to fill and verify before publication.
	pub(crate) const fn directory(&self) -> &DestinationDirectory<'root> {
		&self.directory
	}

	/// The wrapper's single component relative to the authorized destination.
	pub(crate) const fn name(&self) -> &HostName {
		&self.name
	}

	/// Whether the filesystem reported a successful rename to the requested name.
	pub(crate) const fn is_published(&self) -> bool {
		matches!(
			self.state,
			State::Renamed | State::Cleaned { published: true }
		)
	}

	/// Supplies an attachment-checked path to an explicitly requested checker.
	pub(crate) fn path_for_check(&self) -> Result<PathBuf, HostError> {
		self.check_staging()?;
		self.directory.attached_path()
	}

	/// Atomically publish the already verified directory at the absent target.
	///
	/// A successful rename remains published if subsequent identity checks or
	/// removal of the private wrapper fail. Read [`Self::is_published`] on error.
	pub(crate) fn publish(&mut self) -> Result<(), HostError> {
		self.check_staging()?;
		fs::renameat_with(
			self.private.descriptor(),
			self.target.as_str(),
			self.parent.descriptor(),
			self.target.as_str(),
			RenameFlags::NOREPLACE,
		)
		.map_err(|error| match error {
			Errno::EXIST | Errno::NOTEMPTY => HostError::Conflict,
			Errno::NOSYS | Errno::NOTSUP | Errno::INVAL => {
				HostError::NoClobberUnavailable(error.into())
			}
			_ => HostError::CommitDirectoryStage(error.into()),
		})?;
		self.state = State::Renamed;
		self.check_publication()
			.map_err(|error| HostError::PublishedDirectoryUnavailable(Box::new(error)))?;
		self.cleanup()
	}

	fn check_staging(&self) -> Result<(), HostError> {
		if !matches!(self.state, State::Staging) {
			return Err(HostError::DirectoryStageChanged);
		}
		self.directory.check_attachment()?;
		owner_only(&self.private)
	}

	/// Remove only empty stage directories whose attachments still match.
	///
	/// The caller owns the contents created inside the stage. Post-publication
	/// cleanup is limited to the private wrapper.
	pub(crate) fn cleanup(&mut self) -> Result<(), HostError> {
		if matches!(self.state, State::Cleaned { .. }) {
			return Ok(());
		}
		if matches!(self.state, State::Staging) {
			remove_empty(&self.private, &self.target, &self.directory)?;
			self.state = State::EmptyWrapper;
		}
		remove_empty(self.parent, &self.name, &self.private)?;
		self.state = State::Cleaned {
			published: self.is_published(),
		};
		Ok(())
	}

	fn check_publication(&self) -> Result<(), HostError> {
		self.private.check_attachment()?;
		let name = self.target.as_str().as_ref();
		let expected = fs::fstat(self.directory.descriptor()).map_err(inspect)?;
		let target = stat_entry(self.parent.descriptor(), name).map_err(inspect)?;
		let old = stat_entry(self.private.descriptor(), name).map_err(inspect)?;
		if old.is_some()
			|| !target.is_some_and(|target| {
				identity(&target) == identity(&expected)
					&& FileType::from_raw_mode(target.st_mode) == FileType::Directory
			}) {
			return Err(HostError::DirectoryStageChanged);
		}
		Ok(())
	}
}

impl Drop for DirectoryStage<'_, '_> {
	fn drop(&mut self) {
		let _ = self.cleanup();
	}
}

fn owner_only(directory: &DestinationDirectory<'_>) -> Result<(), HostError> {
	let stat = fs::fstat(directory.descriptor()).map_err(inspect)?;
	if Mode::from_raw_mode(stat.st_mode) == PRIVATE_DIRECTORY {
		Ok(())
	} else {
		Err(HostError::DirectoryStageChanged)
	}
}

fn remove_empty(
	parent: &DestinationDirectory<'_>,
	name: &HostName,
	directory: &DestinationDirectory<'_>,
) -> Result<(), HostError> {
	directory.check_attachment()?;
	fs::unlinkat(parent.descriptor(), name.as_str(), AtFlags::REMOVEDIR)
		.map_err(|error| HostError::RemoveDirectoryStage(error.into()))
}

fn unavailable(name: HostName, source: HostError) -> HostError {
	HostError::DirectoryStageUnavailable {
		name,
		source: Box::new(source),
	}
}
