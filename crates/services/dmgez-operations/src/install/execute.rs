use std::{collections::TryReserveError, error::Error as StdError, io, path::Path};

use dmgez_vfs::volume::{NodeHandle, ViewError, VolumeBackend, VolumeView};
use thiserror::Error;

use crate::extract::{
	HostName,
	host::{DestinationDirectory, DirectoryStage, HostError},
};

use super::{CopyContent, CopyPlan, CopySummary};

mod native;

#[cfg(target_os = "macos")]
pub use native::existing::{ExistingBundleError, verify_existing_bundle};

/// Application metadata that the host copy profile cannot reproduce.
#[derive(Debug, Error)]
#[error("application metadata cannot be represented in this host profile")]
pub struct HostMetadataError;

impl<Id> CopyPlan<Id> {
	/// Checks support for the admitted metadata on this host platform.
	///
	/// # Errors
	/// Returns an error for metadata with no implementation in this profile.
	/// A specific destination filesystem may impose further restrictions.
	pub fn require_host_metadata(&self) -> Result<(), HostMetadataError> {
		std::iter::once(self.root())
			.chain(self.entries())
			.all(native::host_supports)
			.then_some(())
			.ok_or(HostMetadataError)
	}
}

/// Failed installation with its observable destination effects.
#[derive(Debug, Error)]
#[error("application installation did not finish")]
pub struct InstallError<E: StdError + 'static> {
	published: bool,
	residual_name: Option<HostName>,
	cleanup_error: Option<HostError>,
	#[source]
	failure: InstallFailure<E>,
}

impl<E: StdError + 'static> InstallError<E> {
	/// Whether the host reported a successful rename to the requested name.
	#[must_use]
	pub const fn published(&self) -> bool {
		self.published
	}

	/// Private wrapper basename that may remain beneath the destination.
	#[must_use]
	pub const fn residual_name(&self) -> Option<&HostName> {
		self.residual_name.as_ref()
	}

	#[must_use]
	pub const fn cleanup_error(&self) -> Option<&HostError> {
		self.cleanup_error.as_ref()
	}

	#[must_use]
	pub const fn failure(&self) -> &InstallFailure<E> {
		&self.failure
	}
}

/// Source verification, staging, or final publication failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InstallFailure<E: StdError + 'static> {
	#[error("application installation was cancelled")]
	Cancelled,
	#[error("could not revalidate the held source image")]
	SourceIdentity(#[source] io::Error),
	#[error("could not reopen an admitted source stream")]
	Source(#[from] ViewError<E>),
	#[error("an admitted source stream changed or became unavailable")]
	SourceChanged,
	#[error(transparent)]
	UnsupportedMetadata(#[from] HostMetadataError),
	#[error("staged object does not match the source bytes, metadata, or name binding")]
	ReadBack,
	#[error("the requested prepublication application check failed")]
	Prepublication(#[source] io::Error),
	#[error("could not reserve application staging memory")]
	Reserve(#[from] TryReserveError),
	#[error(transparent)]
	Host(#[from] HostError),
	#[error("application staging I/O failed")]
	Io(#[from] io::Error),
}

/// Stage, independently read back, and publish one verified app bundle.
///
/// The destination must be a stable local namespace controlled by the caller,
/// with no inherited ACL that grants uncooperative access to the private stage.
/// The source verifier must check the immutable image held by `view`. File and
/// directory modes, data, admitted attributes, and relative symlink targets
/// are verified before one no-clobber rename. On error a nonempty private
/// stage may remain for explicit inspection. This does not run the application.
///
/// # Errors
/// Reports source changes, cancellation, metadata and I/O errors, destination
/// conflicts, and whether any publication or staging residue was observed.
pub fn execute_copy<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	plan: &CopyPlan<B::Id>,
	destination: &DestinationDirectory<'_>,
	verify_source: impl FnMut() -> io::Result<()>,
	cancelled: impl FnMut() -> bool,
) -> Result<CopySummary, InstallError<B::Error>> {
	execute(
		view,
		plan,
		destination,
		verify_source,
		cancelled,
		None::<fn(&Path) -> io::Result<()>>,
	)
}

/// Stage a copy, run an explicit read-only check, then verify and publish it.
///
/// The checker receives the attachment-checked path of the private bundle.
/// The caller must keep the containing namespace stable and restrict access
/// to the stage throughout the operation. The completed tree is checked again
/// against the source before the no-clobber publication. The host may attach
/// its own provenance attribute on macOS. A rejected nonempty stage may
/// remain under its reported private name.
///
/// # Errors
/// Reports the same failures as [`execute_copy`] and any error from the check
/// or the subsequent staged read-back.
pub fn execute_copy_with_check<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	plan: &CopyPlan<B::Id>,
	destination: &DestinationDirectory<'_>,
	verify_source: impl FnMut() -> io::Result<()>,
	cancelled: impl FnMut() -> bool,
	check: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<CopySummary, InstallError<B::Error>> {
	execute(
		view,
		plan,
		destination,
		verify_source,
		cancelled,
		Some(check),
	)
}

fn execute<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	plan: &CopyPlan<B::Id>,
	destination: &DestinationDirectory<'_>,
	mut verify_source: impl FnMut() -> io::Result<()>,
	mut cancelled: impl FnMut() -> bool,
	check: Option<impl FnOnce(&Path) -> io::Result<()>>,
) -> Result<CopySummary, InstallError<B::Error>> {
	let name = plan.application().destination_name();
	let mut run = Run {
		view,
		verify_source: &mut verify_source,
		cancelled: &mut cancelled,
	};
	let children = run.prepare(plan).map_err(before_stage)?;
	let mut stage = destination.stage_directory(name).map_err(|error| {
		let mut failure = before_stage(error.into());
		if let InstallFailure::Host(HostError::DirectoryStageUnavailable { name, .. }) =
			&failure.failure
		{
			failure.residual_name = Some(name.clone());
		}
		failure
	})?;
	let result = run
		.fill(plan, &children, &stage)
		.and_then(|()| {
			if let Some(check) = check {
				run.check_ready()?;
				let path = stage.path_for_check()?;
				check(&path).map_err(InstallFailure::Prepublication)?;
				run.verify_tree(plan, &children, stage.directory())?;
			}
			Ok(())
		})
		.and_then(|()| run.check_ready())
		.and_then(|()| stage.publish().map_err(Into::into));
	if let Err(failure) = result {
		let cleanup_error = stage.cleanup().err();
		return Err(InstallError {
			published: stage.is_published(),
			residual_name: cleanup_error.as_ref().map(|_| stage.name().clone()),
			cleanup_error,
			failure,
		});
	}
	Ok(plan.summary())
}

fn before_stage<E: StdError + 'static>(failure: InstallFailure<E>) -> InstallError<E> {
	InstallError {
		published: false,
		residual_name: None,
		cleanup_error: None,
		failure,
	}
}

struct Run<'view, 'callbacks, B> {
	view: &'view mut VolumeView<B>,
	verify_source: &'callbacks mut dyn FnMut() -> io::Result<()>,
	cancelled: &'callbacks mut dyn FnMut() -> bool,
}

impl<B: VolumeBackend> Run<'_, '_, B> {
	fn prepare(
		&mut self,
		plan: &CopyPlan<B::Id>,
	) -> Result<Vec<Vec<usize>>, InstallFailure<B::Error>> {
		self.check_ready()?;
		self.view.stat(plan.application().source())?;
		plan.require_host_metadata()?;
		let mut children = Vec::new();
		let root_index = plan.entries().len();
		let slots = root_index + 1;
		children.try_reserve_exact(slots)?;
		children.resize_with(slots, Vec::new);
		for (index, entry) in plan.application().names().entries().iter().enumerate() {
			let parent = entry.parent.map_or(root_index, |id| id.index());
			children[parent].try_reserve(1)?;
			children[parent].push(index);
		}
		Ok(children)
	}

	fn fill(
		&mut self,
		plan: &CopyPlan<B::Id>,
		children: &[Vec<usize>],
		stage: &DirectoryStage<'_, '_>,
	) -> Result<(), InstallFailure<B::Error>> {
		let root = stage.directory();
		self.fill_children(plan, children, plan.entries().len(), root)?;
		self.finish_directory(plan.application().source(), plan.root(), root)
	}

	fn fill_children(
		&mut self,
		plan: &CopyPlan<B::Id>,
		children: &[Vec<usize>],
		parent_index: usize,
		parent: &DestinationDirectory<'_>,
	) -> Result<(), InstallFailure<B::Error>> {
		let names = plan.application().names().entries();
		for &index in &children[parent_index] {
			self.check_ready()?;
			let name = &names[index];
			let entry = &plan.entries()[index];
			match &entry.content {
				CopyContent::Directory => {
					let directory = parent.create_child(&name.name)?;
					self.fill_children(plan, children, index, &directory)?;
					self.finish_directory(&name.source, entry, &directory)?;
				}
				CopyContent::File { .. } => {
					self.write_file(&name.source, entry, parent, &name.name)?
				}
				CopyContent::Link(target) => {
					native::write_link(parent, &name.name, target, entry.mode)?
				}
			}
		}
		Ok(())
	}

	fn finish_directory(
		&mut self,
		source: &NodeHandle<B::Id>,
		entry: &super::CopyEntry,
		directory: &DestinationDirectory<'_>,
	) -> Result<(), InstallFailure<B::Error>> {
		self.check_ready()?;
		directory.check_attachment()?;
		let fd = directory.descriptor();
		self.write_attributes(source, entry, fd)?;
		native::apply_mode(fd, entry.mode)?;
		native::sync(fd)?;
		Ok(())
	}

	fn check_ready(&mut self) -> Result<(), InstallFailure<B::Error>> {
		if (self.cancelled)() {
			return Err(InstallFailure::Cancelled);
		}
		(self.verify_source)().map_err(InstallFailure::SourceIdentity)
	}
}
