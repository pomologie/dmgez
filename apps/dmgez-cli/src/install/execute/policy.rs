use std::{io, path::Path};

use serde::Serialize;

use super::Arguments;

#[derive(Serialize)]
pub(super) struct Checks {
	#[serde(skip_serializing_if = "Option::is_none")]
	signature_check: Option<Status>,
	#[serde(skip_serializing_if = "Option::is_none")]
	gatekeeper_assessment: Option<Status>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
	Pending,
	#[cfg(all(feature = "signing-native", target_os = "macos"))]
	Passed,
	#[cfg(all(feature = "signing-native", target_os = "macos"))]
	Failed,
	#[cfg(all(feature = "signing-native", target_os = "macos"))]
	Unavailable,
}

impl Status {
	const fn as_str(self) -> &'static str {
		match self {
			Self::Pending => "pending",
			#[cfg(all(feature = "signing-native", target_os = "macos"))]
			Self::Passed => "passed",
			#[cfg(all(feature = "signing-native", target_os = "macos"))]
			Self::Failed => "failed",
			#[cfg(all(feature = "signing-native", target_os = "macos"))]
			Self::Unavailable => "unavailable",
		}
	}
}

impl Checks {
	pub(super) const fn new(arguments: &Arguments) -> Self {
		Self {
			signature_check: if arguments.verify_app_signature {
				Some(Status::Pending)
			} else {
				None
			},
			gatekeeper_assessment: if arguments.assess {
				Some(Status::Pending)
			} else {
				None
			},
		}
	}

	pub(super) const fn requested(&self) -> bool {
		self.signature_check.is_some() || self.gatekeeper_assessment.is_some()
	}

	pub(super) fn write(&self, output: &mut impl io::Write) -> io::Result<()> {
		if let Some(status) = self.signature_check {
			writeln!(output, "signature_check = {}", status.as_str())?;
		}
		if let Some(status) = self.gatekeeper_assessment {
			writeln!(output, "gatekeeper_assessment = {}", status.as_str())?;
		}
		Ok(())
	}

	#[cfg(all(feature = "signing-native", target_os = "macos"))]
	pub(super) fn verify(&mut self, path: &Path) -> io::Result<()> {
		use dmgez_signing::{Backend, verify_application};

		if let Some(status) = &mut self.signature_check {
			let result = verify_application(path, Backend::Native);
			*status = if result.is_ok() {
				Status::Passed
			} else {
				Status::Failed
			};
			result.map_err(io::Error::other)?;
		}
		if let Some(status) = &mut self.gatekeeper_assessment {
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
fn assess(path: &Path, status: &mut Status) -> io::Result<()> {
	use dmgez_signing::{
		ArtifactKind, NativeAssessmentStatus, NativeAssessmentTool, assess_native_policy,
	};

	let report = assess_native_policy(path).map_err(|failure| {
		*status = Status::Unavailable;
		io::Error::other(failure)
	})?;
	match (report.kind, report.tool, report.outcome) {
		(
			Some(ArtifactKind::AppBundle),
			Some(NativeAssessmentTool::SyspolicyCheck),
			NativeAssessmentStatus::Accepted,
		) => {
			*status = Status::Passed;
			Ok(())
		}
		(_, _, NativeAssessmentStatus::Rejected) => {
			*status = Status::Failed;
			Err(io::Error::new(
				io::ErrorKind::PermissionDenied,
				"Gatekeeper rejected the staged application",
			))
		}
		(_, _, NativeAssessmentStatus::Unavailable(reason)) => {
			*status = Status::Unavailable;
			Err(io::Error::new(
				io::ErrorKind::Unsupported,
				format!("Gatekeeper assessment unavailable: {}", reason.as_str()),
			))
		}
		_ => {
			*status = Status::Unavailable;
			Err(io::Error::other(
				"Gatekeeper response did not establish acceptance for the staged application",
			))
		}
	}
}
