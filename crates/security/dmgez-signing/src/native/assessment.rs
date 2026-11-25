use std::{ffi::OsStr, io, path::Path};

use serde::Deserialize;

use crate::{
	ArtifactKind, NativeAssessmentError, NativeAssessmentReport, NativeAssessmentStatus,
	NativeAssessmentTool, NativeAssessmentUnavailable, assessment::ASSESSMENT_SCHEMA_VERSION,
};

use super::{artifact::Artifact, runner};

#[cfg(test)]
mod tests;

const STATUS_OPTIONS: &[&str] = &["--status"];
const DISK_OPTIONS: &[&str] = &[
	"--assess",
	"--type",
	"open",
	"--context",
	"context:primary-signature",
	"--ignore-cache",
	"--no-cache",
	"--raw",
];
const APP_OPTIONS: &[&str] = &["distribution"];
const POLICY_ENABLED: &[u8] = b"assessments enabled";
const POLICY_DISABLED: &[u8] = b"assessments disabled";
const APP_PASSED: &[u8] = b"App passed all pre-distribution checks and is ready for distribution.";
const APP_FAILED: &[u8] = b"App has failed one or more pre-distribution checks.";
const ASSESSMENT_DENIED: i32 = 3;

pub(crate) fn check(path: &Path) -> Result<NativeAssessmentReport, NativeAssessmentError> {
	let artifact = Artifact::open(path)?;
	check_with(&artifact, runner::assess)
}

fn check_with(
	artifact: &Artifact,
	mut run: impl FnMut(NativeAssessmentTool, &[&OsStr]) -> Result<runner::Output, runner::Error>,
) -> Result<NativeAssessmentReport, NativeAssessmentError> {
	let (tool, options) = match artifact.kind() {
		ArtifactKind::AppBundle => (NativeAssessmentTool::SyspolicyCheck, APP_OPTIONS),
		ArtifactKind::DiskImage => (NativeAssessmentTool::Spctl, DISK_OPTIONS),
	};
	let outcome = match policy_state(artifact, &mut run)? {
		Ok(()) => {
			let mut arguments: Vec<_> = options.iter().map(OsStr::new).collect();
			// Artifact admission supplies an absolute path as a single operand.
			arguments.push(artifact.path().as_os_str());
			match run_checked(artifact, &mut run, tool, &arguments)? {
				Some(output) => match tool {
					NativeAssessmentTool::Spctl => disk_verdict(&output),
					NativeAssessmentTool::SyspolicyCheck => app_verdict(&output),
				},
				None => {
					NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::MissingTool)
				}
			}
		}
		Err(reason) => NativeAssessmentStatus::Unavailable(reason),
	};
	// An app's distribution tool does not expose whether an otherwise failing
	// Gatekeeper decision was overridden. Require assessments to stay enabled.
	let outcome = if tool == NativeAssessmentTool::SyspolicyCheck
		&& outcome == NativeAssessmentStatus::Accepted
	{
		match policy_state(artifact, &mut run)? {
			Ok(()) => outcome,
			Err(reason) => NativeAssessmentStatus::Unavailable(reason),
		}
	} else {
		outcome
	};
	Ok(NativeAssessmentReport {
		schema_version: ASSESSMENT_SCHEMA_VERSION,
		artifact: artifact.path().to_string_lossy().into_owned(),
		kind: Some(artifact.kind()),
		tool: Some(tool),
		outcome,
	})
}

fn policy_state(
	artifact: &Artifact,
	run: &mut impl FnMut(NativeAssessmentTool, &[&OsStr]) -> Result<runner::Output, runner::Error>,
) -> Result<Result<(), NativeAssessmentUnavailable>, NativeAssessmentError> {
	let options: Vec<_> = STATUS_OPTIONS.iter().map(OsStr::new).collect();
	let output = run_checked(artifact, run, NativeAssessmentTool::Spctl, &options)?;
	Ok(match output {
		Some(output) if output.status.success() && output.stderr.is_empty() => {
			match output.stdout.trim_ascii() {
				POLICY_ENABLED => Ok(()),
				POLICY_DISABLED => Err(NativeAssessmentUnavailable::PolicyDisabled),
				_ => Err(NativeAssessmentUnavailable::UnknownToolResponse),
			}
		}
		Some(_) => Err(NativeAssessmentUnavailable::UnknownToolResponse),
		None => Err(NativeAssessmentUnavailable::MissingTool),
	})
}

fn run_checked(
	artifact: &Artifact,
	run: &mut impl FnMut(NativeAssessmentTool, &[&OsStr]) -> Result<runner::Output, runner::Error>,
	tool: NativeAssessmentTool,
	args: &[&OsStr],
) -> Result<Option<runner::Output>, NativeAssessmentError> {
	artifact.validate()?;
	let result = run(tool, args);
	artifact.validate()?;
	match result {
		Ok(output) => Ok(Some(output)),
		Err(runner::Error::Spawn(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
		Err(error) => Err(NativeAssessmentError::Tool(Box::new(error))),
	}
}

// These result keys are named in Apple Security's SecAssessment API.
#[derive(Deserialize)]
struct DiskVerdict {
	#[serde(rename = "assessment:verdict")]
	accepted: bool,
	#[serde(rename = "assessment:authority")]
	authority: Option<Authority>,
	#[serde(rename = "assessment:error")]
	error: Option<plist::Value>,
}

#[derive(Deserialize)]
struct Authority {
	#[serde(rename = "assessment:authority:override")]
	override_reason: Option<plist::Value>,
	#[serde(rename = "assessment:authority:verdict")]
	original_verdict: Option<bool>,
}

fn disk_verdict(output: &runner::Output) -> NativeAssessmentStatus {
	let unavailable = NativeAssessmentStatus::Unavailable;
	let Ok(verdict) = plist::from_bytes::<DiskVerdict>(&output.stdout) else {
		return unavailable(NativeAssessmentUnavailable::UnknownToolResponse);
	};
	if verdict.authority.is_some_and(|authority| {
		authority.override_reason.is_some()
			|| authority.original_verdict == Some(false) && verdict.accepted
	}) {
		return unavailable(NativeAssessmentUnavailable::OverriddenVerdict);
	}
	if verdict.error.is_some() {
		return unavailable(NativeAssessmentUnavailable::UnknownToolResponse);
	}
	match (output.status.code(), verdict.accepted) {
		(Some(0), true) if output.stderr.is_empty() => NativeAssessmentStatus::Accepted,
		(Some(ASSESSMENT_DENIED), false) => NativeAssessmentStatus::Rejected,
		_ => unavailable(NativeAssessmentUnavailable::UnknownToolResponse),
	}
}

fn app_verdict(output: &runner::Output) -> NativeAssessmentStatus {
	if output.status.success()
		&& output.stdout.trim_ascii() == APP_PASSED
		&& output.stderr.is_empty()
	{
		return NativeAssessmentStatus::Accepted;
	}
	if output.status.code().is_some_and(|code| code != 0)
		&& [&output.stdout, &output.stderr]
			.into_iter()
			.any(|stream| stream.split(|&byte| byte == b'\n').next() == Some(APP_FAILED))
	{
		return NativeAssessmentStatus::Rejected;
	}
	NativeAssessmentStatus::Unavailable(NativeAssessmentUnavailable::UnknownToolResponse)
}
