use std::path::Path;

use super::{Check, Status};

#[cfg(not(feature = "signing-native"))]
const SIGNING_FEATURE: &str = "signing-native";
#[cfg(not(feature = "notarization-native"))]
const TICKET_FEATURE: &str = "notarization-native";

pub(super) fn signature(path: &Path) -> Check<&'static str> {
	#[cfg(feature = "signing-native")]
	{
		use dmgez_signing::{
			ArtifactKind, Backend, Operation, Status as NativeStatus, check_signature,
		};
		match check_signature(path, Operation::Verify, Backend::Native) {
			Ok(report) => {
				let status = match report.status {
					NativeStatus::Verified if report.kind == Some(ArtifactKind::DiskImage) => {
						Status::Passed
					}
					NativeStatus::Unsigned | NativeStatus::Failed => Status::Failed,
					NativeStatus::Unavailable => Status::Unavailable,
					_ => Status::Error,
				};
				Check::evidence(status, report.status.as_str())
			}
			Err(error) => Check::detail(Status::Error, error),
		}
	}
	#[cfg(not(feature = "signing-native"))]
	{
		let _ = path;
		Check::detail(Status::Unavailable, SIGNING_FEATURE)
	}
}

pub(super) fn ticket(path: &Path) -> Check<&'static str> {
	#[cfg(feature = "notarization-native")]
	{
		use dmgez_signing::notarization::{TicketStatus, check_ticket};
		match check_ticket(path) {
			Ok(result) => {
				let status = match result {
					TicketStatus::Verified => Status::Passed,
					TicketStatus::Missing | TicketStatus::Invalid => Status::Failed,
					TicketStatus::Unavailable => Status::Unavailable,
					_ => Status::Error,
				};
				Check::evidence(status, result.as_str())
			}
			Err(error) => Check::detail(Status::Error, error),
		}
	}
	#[cfg(not(feature = "notarization-native"))]
	{
		let _ = path;
		Check::detail(Status::Unavailable, TICKET_FEATURE)
	}
}

pub(super) fn assess(path: &Path) -> Check<&'static str> {
	#[cfg(feature = "signing-native")]
	{
		use dmgez_signing::{
			ArtifactKind, NativeAssessmentStatus, NativeAssessmentTool, assess_native_policy,
		};
		match assess_native_policy(path) {
			Ok(report) => match report.outcome {
				NativeAssessmentStatus::Accepted
					if report.kind == Some(ArtifactKind::DiskImage)
						&& report.tool == Some(NativeAssessmentTool::Spctl) =>
				{
					Check::evidence(Status::Passed, report.outcome.as_str())
				}
				NativeAssessmentStatus::Rejected => {
					Check::evidence(Status::Failed, report.outcome.as_str())
				}
				NativeAssessmentStatus::Unavailable(reason) => {
					Check::detail(Status::Unavailable, reason.as_str())
				}
				_ => Check::new(Status::Error),
			},
			Err(error) => Check::detail(Status::Error, error),
		}
	}
	#[cfg(not(feature = "signing-native"))]
	{
		let _ = path;
		Check::detail(Status::Unavailable, SIGNING_FEATURE)
	}
}
