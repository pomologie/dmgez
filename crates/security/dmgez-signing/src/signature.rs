use std::{
	io,
	path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{Backend, Operation, PolicyAssessment, Report, Status, report::SCHEMA_VERSION};

/// Reason a path failed artifact admission.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum ArtifactProblem {
	#[error("expected an application bundle directory or a UDIF disk image file")]
	Kind,
	#[error("disk image lacks an admitted UDIF v4 trailer header")]
	UdifHeader,
	#[error("native signature tool did not identify the artifact as a disk image")]
	NativeFormat,
}

/// Failure outside the tool's signature outcome.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("could not read artifact {path:?}")]
	ArtifactIo {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("could not admit artifact {path:?}: {problem}")]
	InvalidArtifact {
		path: PathBuf,
		problem: ArtifactProblem,
	},
	#[error("artifact {path:?} changed during the signature check")]
	ArtifactChanged { path: PathBuf },
	#[error("could not run the native signature tool: {source}")]
	Tool {
		#[source]
		source: Box<dyn std::error::Error + Send + Sync>,
	},
	#[error("native signature tool exited unexpectedly ({status})")]
	ToolExit { status: std::process::ExitStatus },
	#[error("could not check the disk image with the Rust backend: {0}")]
	Rust(#[source] Box<crate::SignError>),
}

/// Inspect or verify an app bundle or a UDIF image using an available backend.
///
/// Backend selection occurs before artifact access. The Rust profile checks
/// the CodeDirectory integrity of an admitted ad hoc disk image. With the native
/// feature on macOS, `Auto` uses the operating system's signature checker.
/// Otherwise, `Auto` can use the compiled Rust profile. Platform policy and
/// filesystem checks are separate operations.
///
/// # Errors
///
/// Returns an error for an invalid or inaccessible artifact, source changes,
/// or a failure to run the selected backend.
pub fn check_signature(
	path: &Path,
	operation: Operation,
	backend: Backend,
) -> Result<Report, Error> {
	if backend == Backend::Rust {
		#[cfg(all(
			feature = "rust",
			any(target_os = "linux", target_os = "macos", windows)
		))]
		return crate::signing::apple::check(path, operation, backend);
		#[cfg(not(all(
			feature = "rust",
			any(target_os = "linux", target_os = "macos", windows)
		)))]
		return Ok(unavailable(
			path,
			operation,
			backend,
			"Rust signature backend is unavailable",
		));
	}
	#[cfg(all(feature = "native", target_os = "macos"))]
	{
		crate::native::check(path, operation, backend)
	}
	#[cfg(not(all(feature = "native", target_os = "macos")))]
	{
		#[cfg(all(
			feature = "rust",
			any(target_os = "linux", target_os = "macos", windows)
		))]
		if backend == Backend::Auto {
			return crate::signing::apple::check(path, operation, backend);
		}
		Ok(unavailable(
			path,
			operation,
			backend,
			"native signature backend requires macOS and the native feature",
		))
	}
}

pub(super) fn report_base(path: &Path, operation: Operation, backend: Backend) -> Report {
	Report {
		schema_version: SCHEMA_VERSION,
		operation,
		requested_backend: backend,
		backend: None,
		status: Status::Unavailable,
		artifact: path.to_string_lossy().into_owned(),
		kind: None,
		assessment: PolicyAssessment::NotPerformed,
		diagnostics: None,
	}
}

#[cfg(not(all(feature = "native", target_os = "macos", feature = "rust")))]
fn unavailable(path: &Path, operation: Operation, backend: Backend, reason: &str) -> Report {
	let mut report = report_base(path, operation, backend);
	report.diagnostics = Some(reason.to_owned());
	report
}
