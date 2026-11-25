//! Extract a bounded projection from an admitted filesystem view.

use std::{
	error::Error as StdError,
	hash::Hash,
	io,
	os::unix::ffi::OsStrExt,
	path::{Path, PathBuf, absolute},
};

use dmgez_inspector::volume_queries;
use dmgez_operations::extract::{
	ExecutionError, ExtractionLimits, HostName, NameError, PlanError, execute,
	host::{HostError, StableDestination},
	prepare,
};
use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use thiserror::Error;

pub use dmgez_operations::extract::{PlanReport, Progress, Selection};

const PATH_SEPARATOR: u8 = b'/';
const CURRENT_COMPONENT: &[u8] = b".";
const PARENT_COMPONENT: &[u8] = b"..";

/// A requested destination with both its original spelling and absolute path.
pub struct OutputPath<'path> {
	requested: &'path Path,
	path: PathBuf,
}

impl<'path> OutputPath<'path> {
	/// Resolve the path against the working directory without following links.
	///
	/// # Errors
	/// Returns an error if the absolute path cannot be constructed.
	pub fn new(requested: &'path Path) -> io::Result<Self> {
		Ok(Self {
			requested,
			path: absolute(requested)?,
		})
	}

	#[must_use]
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Pin the parent of an absent output name before opening the source image.
	///
	/// # Errors
	/// Rejects unsafe or occupied output names, inadmissible parent directories,
	/// and payload limits above the extraction profile's maximum.
	pub fn admit(
		&self,
		selection: Selection,
		max_bytes: Option<u64>,
	) -> Result<Destination, DestinationError> {
		let mut components = self
			.requested
			.as_os_str()
			.as_bytes()
			.rsplit(|&byte| byte == PATH_SEPARATOR);
		let last = match selection {
			Selection::DataTree => components.find(|component| !component.is_empty()),
			_ => components.next().filter(|component| !component.is_empty()),
		};
		if !matches!(last, Some(component) if !matches!(component, CURRENT_COMPONENT | PARENT_COMPONENT))
		{
			return Err(DestinationError::InvalidPath);
		}
		let parent = self.path.parent().ok_or(DestinationError::InvalidPath)?;
		let name = self
			.path
			.file_name()
			.and_then(|name| name.to_str())
			.ok_or(DestinationError::InvalidPath)?;
		let mut limits = ExtractionLimits::default();
		if let Some(max_bytes) = max_bytes {
			if max_bytes > limits.max_total_bytes {
				return Err(DestinationError::PayloadLimit {
					maximum: limits.max_total_bytes,
				});
			}
			limits.max_total_bytes = max_bytes;
		}
		let name = HostName::new(name, limits.profile, limits.names.max_component_bytes)?;
		let parent = StableDestination::open(parent)?;
		parent.directory().require_absent(&name)?;
		Ok(Destination {
			parent,
			name,
			selection,
			limits,
		})
	}
}

/// A pinned destination bound to the admitted projection and limits.
pub struct Destination {
	parent: StableDestination,
	name: HostName,
	selection: Selection,
	limits: ExtractionLimits,
}

/// Why the requested destination or extraction budget was refused.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DestinationError {
	#[error("extraction output must name an absent entry under an existing parent")]
	InvalidPath,
	#[error("the extraction payload limit cannot exceed {maximum} bytes")]
	PayloadLimit { maximum: u64 },
	#[error(transparent)]
	Name(#[from] NameError),
	#[error(transparent)]
	Host(#[from] HostError),
}

/// Evidence retained even when publication or a later source check fails.
#[derive(Debug, Default)]
pub struct Evidence {
	pub plan: Option<PlanReport>,
	pub progress: Option<Progress>,
}

/// Why planning or publication could not complete.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error<E: StdError + 'static> {
	#[error(transparent)]
	Query(#[from] volume_queries::Error<E>),
	#[error(transparent)]
	Plan(#[from] PlanError<E>),
	#[error(transparent)]
	SourceIdentity(#[from] io::Error),
	#[error(transparent)]
	Destination(#[from] HostError),
	#[error(transparent)]
	Publication(#[from] ExecutionError<E>),
}

impl Destination {
	/// Plan the selected source path, then verify or publish it at this destination.
	///
	/// # Errors
	/// Returns lookup, planning, source identity or publication failures.
	/// `evidence` records the plan and any reported progress up to that point.
	pub fn project<B: VolumeBackend>(
		&self,
		view: &mut VolumeView<B>,
		source_path: &str,
		dry_run: bool,
		mut verify_source: impl FnMut() -> io::Result<()>,
		mut cancelled: impl FnMut() -> bool,
		evidence: &mut Evidence,
	) -> Result<(), Error<B::Error>>
	where
		B::Id: Eq + Hash,
	{
		*evidence = Evidence::default();
		let node = volume_queries::resolve_node(view, source_path)?;
		let plan = prepare(view, &node, self.selection, self.limits, &mut cancelled)?;
		evidence.plan = Some(*plan.report());
		let directory = self.parent.directory();
		if dry_run {
			verify_source()?;
			directory.require_absent(&self.name)?;
			return Ok(());
		}
		let publication = execute(
			view,
			&plan,
			&directory,
			&self.name,
			verify_source,
			cancelled,
		);
		match publication {
			Ok(progress) => {
				evidence.progress = Some(progress);
				Ok(())
			}
			Err(error) => {
				evidence.progress = Some(error.progress());
				Err(error.into())
			}
		}
	}
}
