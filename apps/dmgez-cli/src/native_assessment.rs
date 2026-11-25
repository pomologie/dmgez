use std::{path::Path, process::ExitCode};

use crate::CliResult;

pub(super) const PROFILE: &str = "macos-release";

pub(super) fn run(
	path: &Path,
	folder: Option<&str>,
	suppressions: &[String],
	json: bool,
) -> CliResult<ExitCode> {
	if folder.is_some() || !suppressions.is_empty() {
		return Err(format!(
			"{PROFILE} assesses an artifact and does not accept layout options or suppressions"
		)
		.into());
	}
	run_assessment(path, json)
}

#[cfg(not(feature = "signing-native"))]
fn run_assessment(_path: &Path, _json: bool) -> CliResult<ExitCode> {
	Err(format!("feature not compiled: {}", crate::signing::NATIVE_FEATURE).into())
}

#[cfg(feature = "signing-native")]
fn run_assessment(path: &Path, json: bool) -> CliResult<ExitCode> {
	use std::io::Write;

	use dmgez_signing::{NativeAssessmentReport, NativeAssessmentStatus, assess_native_policy};
	use serde::Serialize;

	use crate::{
		REPORT_ERROR_EXIT_CODE,
		report_output::{EscapedText, write_report},
	};

	const REJECTED_EXIT_CODE: u8 = 1;

	#[derive(Serialize)]
	struct Report {
		profile: &'static str,
		assessment: NativeAssessmentReport,
	}

	let assessment = assess_native_policy(path)?;
	let status = assessment.outcome;
	let report = Report {
		profile: PROFILE,
		assessment,
	};
	write_report(json, &report, |stdout| {
		writeln!(stdout, "profile = {}", report.profile)?;
		writeln!(
			stdout,
			"artifact = {}",
			EscapedText(&report.assessment.artifact)
		)?;
		writeln!(stdout, "assessment = {}", status.as_str())?;
		if let Some(tool) = report.assessment.tool {
			writeln!(stdout, "assessment_tool = {}", tool.as_str())?;
		}
		if let NativeAssessmentStatus::Unavailable(reason) = status {
			writeln!(stdout, "assessment_unavailable = {}", reason.as_str())?;
		}
		Ok(())
	})?;
	Ok(match status {
		NativeAssessmentStatus::Accepted => ExitCode::SUCCESS,
		NativeAssessmentStatus::Rejected => ExitCode::from(REJECTED_EXIT_CODE),
		_ => ExitCode::from(REPORT_ERROR_EXIT_CODE),
	})
}
