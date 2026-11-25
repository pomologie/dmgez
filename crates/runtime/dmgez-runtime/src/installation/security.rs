use std::{io, path::Path};

use thiserror::Error;

/// A requested prepublication check's observed result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CheckStatus {
	Pending,
	Passed,
	Failed,
	Unavailable,
}

/// Native checks requested for the staged bundle, with their observed results.
#[derive(Debug)]
pub struct SecurityChecks {
	signature: Option<CheckStatus>,
	assessment: Option<CheckStatus>,
}

/// Native checks were requested on a build without a supported backend.
#[derive(Debug, Error)]
#[error(
	"application signature verification and Gatekeeper assessment require macOS and native signing support"
)]
pub struct SecurityUnavailable;

impl SecurityChecks {
	#[must_use]
	pub const fn new(signature: bool, assessment: bool) -> Self {
		Self {
			signature: if signature {
				Some(CheckStatus::Pending)
			} else {
				None
			},
			assessment: if assessment {
				Some(CheckStatus::Pending)
			} else {
				None
			},
		}
	}

	#[must_use]
	pub const fn signature(&self) -> Option<CheckStatus> {
		self.signature
	}

	#[must_use]
	pub const fn assessment(&self) -> Option<CheckStatus> {
		self.assessment
	}

	pub(super) const fn requested(&self) -> bool {
		self.signature.is_some() || self.assessment.is_some()
	}

	pub(super) fn reset(&mut self) {
		*self = Self::new(self.signature.is_some(), self.assessment.is_some());
	}

	pub(super) fn require_available(&self) -> Result<(), SecurityUnavailable> {
		if self.requested() && !cfg!(all(feature = "signing-native", target_os = "macos")) {
			return Err(SecurityUnavailable);
		}
		Ok(())
	}

	#[cfg(all(feature = "signing-native", target_os = "macos"))]
	pub(super) fn verify(&mut self, path: &Path) -> io::Result<()> {
		use dmgez_signing::{Backend, verify_application};

		if let Some(status) = &mut self.signature {
			let result = verify_application(path, Backend::Native);
			*status = if result.is_ok() {
				CheckStatus::Passed
			} else {
				CheckStatus::Failed
			};
			result.map_err(io::Error::other)?;
		}
		if let Some(status) = &mut self.assessment {
			assess(path, status)?;
		}
		Ok(())
	}

	#[cfg(not(all(feature = "signing-native", target_os = "macos")))]
	pub(super) fn verify(&mut self, _: &Path) -> io::Result<()> {
		Err(io::ErrorKind::Unsupported.into())
	}
}

#[cfg(all(feature = "signing-native", target_os = "macos"))]
fn assess(path: &Path, status: &mut CheckStatus) -> io::Result<()> {
	use dmgez_signing::{
		ArtifactKind, NativeAssessmentStatus, NativeAssessmentTool, assess_native_policy,
	};

	let report = assess_native_policy(path).map_err(|failure| {
		*status = CheckStatus::Unavailable;
		io::Error::other(failure)
	})?;
	match (report.kind, report.tool, report.outcome) {
		(
			Some(ArtifactKind::AppBundle),
			Some(NativeAssessmentTool::SyspolicyCheck),
			NativeAssessmentStatus::Accepted,
		) => {
			*status = CheckStatus::Passed;
			Ok(())
		}
		(_, _, NativeAssessmentStatus::Rejected) => {
			*status = CheckStatus::Failed;
			Err(io::Error::new(
				io::ErrorKind::PermissionDenied,
				"Gatekeeper rejected the staged application",
			))
		}
		(_, _, NativeAssessmentStatus::Unavailable(reason)) => {
			*status = CheckStatus::Unavailable;
			Err(io::Error::new(
				io::ErrorKind::Unsupported,
				format!("Gatekeeper assessment unavailable: {}", reason.as_str()),
			))
		}
		_ => {
			*status = CheckStatus::Unavailable;
			Err(io::Error::other(
				"Gatekeeper response did not establish acceptance for the staged application",
			))
		}
	}
}
