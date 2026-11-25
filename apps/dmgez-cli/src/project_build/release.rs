use std::{borrow::Cow, error::Error, io, io::Write, path::Path};

use dmgez_runtime::{
	project_build::{BuildEvent, BuildMode, BuildProject},
	project_release::{self, AdmissionError, ReleaseOptions, ReleaseOutcome, ReleaseWorkflowError},
};
use dmgez_signing::release_plan::ReleaseReport;
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

use super::{Arguments, execute::write_dry_run};

const IDENTIFIER_REQUIRED: &str = "disk image signing requires signing.identifier or --identifier";
const NOTARY_RECORD_REQUIRED: &str = "configured notarization requires --notary-record";
const NOTARIZATION_REQUIRED: &str = "--notary-record requires configured notarization";

pub(super) fn run(
	project: &BuildProject,
	arguments: &Arguments,
	working_directory: &Path,
	mode: BuildMode,
	observe: impl Fn(BuildEvent<'_>),
) -> CliResult {
	let outcome = project_release::run(
		project,
		ReleaseOptions {
			mode,
			identifier: arguments.identifier.as_deref(),
			notarization_record: arguments.notary_record.as_deref(),
			working_directory,
		},
		Default::default(),
		observe,
	)
	.map_err(command_error)?;
	match outcome {
		ReleaseOutcome::Planned(summary) => write_dry_run(io::stdout().lock(), &summary)?,
		ReleaseOutcome::Executed(execution) => {
			let report = CommandReport {
				stages: &execution.report,
				published_path: execution
					.published_path
					.as_deref()
					.map(Path::to_string_lossy),
				retained_signed_image: execution
					.retained_signed_image
					.as_deref()
					.map(Path::to_string_lossy),
				notarization_record: execution
					.notarization_record
					.as_deref()
					.map(Path::to_string_lossy),
			};
			report.write(arguments.json)?;
			execution.result.map_err(command_error)?;
		}
	}
	Ok(())
}

fn command_error(error: ReleaseWorkflowError) -> Box<dyn Error> {
	match error {
		ReleaseWorkflowError::Admission(AdmissionError::MissingIdentifier) => {
			IDENTIFIER_REQUIRED.into()
		}
		ReleaseWorkflowError::MissingRecord => NOTARY_RECORD_REQUIRED.into(),
		ReleaseWorkflowError::UnexpectedRecord => NOTARIZATION_REQUIRED.into(),
		error => error.into(),
	}
}

#[derive(Serialize)]
struct CommandReport<'run> {
	#[serde(flatten)]
	stages: &'run ReleaseReport,
	published_path: Option<Cow<'run, str>>,
	retained_signed_image: Option<Cow<'run, str>>,
	notarization_record: Option<Cow<'run, str>>,
}

impl CommandReport<'_> {
	fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |writer| {
			for step in &self.stages.steps {
				writeln!(writer, "{:?} = {:?}", step.stage, step.status)?;
			}
			if let Some(verification) = &self.stages.verified_applications {
				writeln!(writer, "verified applications = {}", verification.count)?;
			}
			if let Some(path) = &self.published_path {
				writeln!(writer, "published = {}", EscapedText(path))?;
			}
			if let Some(path) = &self.retained_signed_image {
				writeln!(writer, "retained signed image = {}", EscapedText(path))?;
			}
			if let Some(path) = &self.notarization_record {
				writeln!(writer, "notarization record = {}", EscapedText(path))?;
			}
			Ok(())
		})
	}
}
