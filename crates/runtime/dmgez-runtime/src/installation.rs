//! Plan and publish a fresh application from an admitted filesystem view.

mod security;

use std::{error::Error as StdError, hash::Hash, io, path::Path};

use dmgez_config::{VirtualPath, VirtualPathError};
use dmgez_operations::{
	extract::{
		HostName,
		host::{HostError, StableDestination},
	},
	install::{
		self, CopyLimits, CopyPlanError, HostMetadataError, InstallError, execute_copy,
		execute_copy_with_check, prepare_copy, prepare_staged_application,
	},
};
use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use thiserror::Error;

pub use dmgez_operations::install::CopySummary;
pub use security::{CheckStatus, SecurityChecks, SecurityUnavailable};

/// One complete mountless application copy request.
pub struct Request<'path> {
	pub source: &'path str,
	pub destination: &'path Path,
	pub dry_run: bool,
	pub max_bytes: Option<u64>,
}

/// Evidence retained even when the copy or a later source check fails.
#[derive(Debug, Default)]
pub struct Evidence {
	pub bundle_name: Option<HostName>,
	pub plan: Option<CopySummary>,
	pub published: bool,
	pub residual_stage_name: Option<HostName>,
}

/// Why the application could not be planned or published.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error<E: StdError + 'static> {
	#[error(transparent)]
	Security(#[from] SecurityUnavailable),
	#[error(transparent)]
	SourcePath(#[from] VirtualPathError),
	#[error(transparent)]
	Application(#[from] install::Error<E>),
	#[error("the application payload limit cannot exceed {maximum} bytes")]
	PayloadLimit { maximum: u64 },
	#[error(transparent)]
	Plan(#[from] CopyPlanError<E>),
	#[error(transparent)]
	Metadata(#[from] HostMetadataError),
	#[error(transparent)]
	Destination(#[from] HostError),
	#[error(transparent)]
	SourceIdentity(#[from] io::Error),
	#[error(transparent)]
	Publication(#[from] InstallError<E>),
}

/// Admit, verify, and optionally publish one application under its stored name.
///
/// The caller must authorize and hold the destination namespace stable, and
/// pass a verifier for the source backing `view`. Optional application checks
/// run inside private staging before the staged tree's final read-back.
///
/// # Errors
/// Rejects unsupported source layout or metadata, payload budget violations,
/// detected source changes, and staging or publication failures. `evidence`
/// records the admitted plan, confirmed publication, and possible residue.
pub fn copy<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	request: &Request<'_>,
	mut verify_source: impl FnMut() -> io::Result<()>,
	mut cancelled: impl FnMut() -> bool,
	checks: &mut SecurityChecks,
	evidence: &mut Evidence,
) -> Result<(), Error<B::Error>>
where
	B::Id: Eq + Hash,
{
	*evidence = Evidence::default();
	checks.reset();
	checks.require_available()?;
	let source = request.source.parse::<VirtualPath>()?;
	let application = prepare_staged_application(view, &source, &mut cancelled)?;
	evidence.bundle_name = Some(application.destination_name().clone());
	let mut limits = CopyLimits::default();
	if let Some(bytes) = request.max_bytes {
		if bytes > limits.max_payload_bytes {
			return Err(Error::PayloadLimit {
				maximum: limits.max_payload_bytes,
			});
		}
		limits.max_payload_bytes = bytes;
	}
	let plan = prepare_copy(view, application, limits, &mut cancelled)?;
	evidence.plan = Some(plan.summary());
	plan.require_host_metadata()?;
	let directory = StableDestination::open(request.destination)?;
	plan.application().require_fresh_destination(&directory)?;
	if request.dry_run {
		verify_source()?;
		return Ok(());
	}
	let destination = directory.directory();
	let result = if checks.requested() {
		execute_copy_with_check(
			view,
			&plan,
			&destination,
			verify_source,
			cancelled,
			|path| checks.verify(path),
		)
	} else {
		execute_copy(view, &plan, &destination, verify_source, cancelled)
	};
	match result {
		Ok(_) => {
			evidence.published = true;
			Ok(())
		}
		Err(error) => {
			evidence.published = error.published();
			evidence.residual_stage_name = error.residual_name().cloned();
			Err(error.into())
		}
	}
}
