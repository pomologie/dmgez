use std::{error::Error as StdError, io, num::NonZeroUsize, ops::Range};

use dmgez_vfs::volume::{ForkKind, NodeHandle, ViewError, VolumeBackend, VolumeView};
use thiserror::Error;

use super::{
	ExtractionPlan, HostName, NameError, Progress,
	host::{DestinationDirectory, ForkMode, ForkWriteError, ForkWriteLimits, HostError},
	plan::{Contents, File, Tree, TreeItem},
};

const CHUNK_BYTES: NonZeroUsize = NonZeroUsize::new(64 * 1024).expect("copy buffer is nonempty");

/// Execution failed. The progress records effects confirmed before the failure.
#[derive(Debug, Error)]
#[error("extraction publication failed")]
pub struct ExecutionError<E: StdError + 'static> {
	progress: Progress,
	#[source]
	failure: ExecutionFailure<E>,
}

impl<E: StdError + 'static> ExecutionError<E> {
	#[must_use]
	pub const fn progress(&self) -> Progress {
		self.progress
	}

	#[must_use]
	pub const fn failure(&self) -> &ExecutionFailure<E> {
		&self.failure
	}
}

/// Cause of an extraction execution failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionFailure<E: StdError + 'static> {
	#[error("output basename does not satisfy the plan's name profile")]
	Name(#[source] NameError),
	#[error(transparent)]
	Host(#[from] HostError),
	#[error("could not revalidate source metadata")]
	Metadata(#[source] ViewError<E>),
	#[error("could not revalidate the extraction source")]
	SourceIdentity(#[source] io::Error),
	#[error("could not reopen the planned source fork")]
	Fork(#[source] ViewError<E>),
	#[error("the planned {0:?} fork became absent")]
	AbsentFork(ForkKind),
	#[error("the planned fork length changed from {planned} to {actual} bytes")]
	LengthChanged { planned: u64, actual: u64 },
	#[error(transparent)]
	Write(#[from] ForkWriteError<E>),
	#[error("extraction was cancelled")]
	Cancelled,
}

/// Publishes a prepared projection under an absent name in an authorized directory.
///
/// The parent and its containing namespace must be stable throughout the operation.
/// Pass a verifier for the held immutable source identity. Directory publication
/// is incremental. Previously published outputs remain after a late failure.
///
/// # Errors
/// Reports rejected destinations, detected source changes, cancellation, and
/// per-entry publication errors with the confirmed effects up to that point.
pub fn execute<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	plan: &ExtractionPlan<B::Id>,
	parent: &DestinationDirectory<'_>,
	name: &HostName,
	mut verify_source: impl FnMut() -> io::Result<()>,
	mut cancelled: impl FnMut() -> bool,
) -> Result<Progress, ExecutionError<B::Error>> {
	let mut run = Run {
		view,
		verify_source: &mut verify_source,
		cancelled: &mut cancelled,
		progress: Progress::default(),
	};
	run.publish(plan, parent, name)
		.map_err(|failure| ExecutionError {
			progress: run.progress,
			failure,
		})?;
	Ok(run.progress)
}

struct Run<'view, 'callbacks, B> {
	view: &'view mut VolumeView<B>,
	verify_source: &'callbacks mut dyn FnMut() -> io::Result<()>,
	cancelled: &'callbacks mut dyn FnMut() -> bool,
	progress: Progress,
}

impl<B: VolumeBackend> Run<'_, '_, B> {
	fn publish(
		&mut self,
		plan: &ExtractionPlan<B::Id>,
		parent: &DestinationDirectory<'_>,
		name: &HostName,
	) -> Result<(), ExecutionFailure<B::Error>> {
		let limits = plan.limits();
		name.check(limits.profile, limits.names.max_component_bytes)
			.map_err(ExecutionFailure::Name)?;
		self.check_ready()?;
		parent.require_absent(name)?;
		self.view
			.stat(&plan.source)
			.map_err(ExecutionFailure::Metadata)?;
		match &plan.contents {
			Contents::File(file) => self.publish_file(parent, name, &plan.source, file),
			Contents::Tree(tree) => {
				let root = self.create_directory(parent, name)?;
				self.publish_entries(&root, tree, tree.children.clone())
			}
		}
	}

	fn publish_entries(
		&mut self,
		parent: &DestinationDirectory<'_>,
		tree: &Tree<B::Id>,
		children: Range<usize>,
	) -> Result<(), ExecutionFailure<B::Error>> {
		for index in children {
			let entry = &tree.entries[index];
			match &entry.item {
				TreeItem::Directory(children) => {
					self.view
						.stat(&entry.source)
						.map_err(ExecutionFailure::Metadata)?;
					let child = self.create_directory(parent, &entry.name)?;
					self.publish_entries(&child, tree, children.clone())?;
				}
				TreeItem::File(file) => {
					self.publish_file(parent, &entry.name, &entry.source, file)?
				}
			}
		}
		Ok(())
	}

	fn create_directory<'root>(
		&mut self,
		parent: &DestinationDirectory<'root>,
		name: &HostName,
	) -> Result<DestinationDirectory<'root>, ExecutionFailure<B::Error>> {
		self.check_ready()?;
		match parent.create_child(name) {
			Ok(child) => {
				self.progress.directories_created += 1;
				Ok(child)
			}
			Err(error) => {
				self.progress.directory_creation_unconfirmed =
					matches!(error, HostError::CreatedDirectoryUnavailable { .. });
				Err(error.into())
			}
		}
	}

	fn publish_file(
		&mut self,
		parent: &DestinationDirectory<'_>,
		name: &HostName,
		source: &NodeHandle<B::Id>,
		file: &File,
	) -> Result<(), ExecutionFailure<B::Error>> {
		self.check_ready()?;
		let limits = ForkWriteLimits {
			max_fork_bytes: file.bytes,
			chunk_bytes: CHUNK_BYTES,
		};
		let bytes = self
			.view
			.with_fork(source, file.fork, |fork| {
				if fork.len() != file.bytes {
					return Err(ExecutionFailure::LengthChanged {
						planned: file.bytes,
						actual: fork.len(),
					});
				}
				Ok(parent.publish_fork_verified(
					name,
					fork,
					limits,
					ForkMode::OwnerReadWrite,
					&mut *self.cancelled,
					&mut *self.verify_source,
				)?)
			})
			.map_err(ExecutionFailure::Fork)?
			.ok_or(ExecutionFailure::AbsentFork(file.fork))??;
		self.progress.files_published += 1;
		// All published forks have the lengths admitted into the checked plan sum.
		self.progress.bytes_published += bytes;
		Ok(())
	}

	fn check_ready(&mut self) -> Result<(), ExecutionFailure<B::Error>> {
		if (self.cancelled)() {
			return Err(ExecutionFailure::Cancelled);
		}
		(self.verify_source)().map_err(ExecutionFailure::SourceIdentity)
	}
}
