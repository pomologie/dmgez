use std::{borrow::Cow, io::Write};

use dmgez_runtime::{
	project_build::{BuildEvent, BuildMode},
	project_edit::{self, EditReport, EditRequest},
};
use serde::Serialize;

use crate::{
	CliResult,
	project_build::{Progress, report_losses},
	project_input,
	report_output::{EscapedText, write_report},
};

use super::Arguments;

const SCHEMA_VERSION: u16 = 1;
const UNSIGNED_TRUST: &str = "unsigned";

pub(super) fn run(arguments: &Arguments) -> CliResult {
	let mode = if arguments.dry_run {
		BuildMode::Plan
	} else {
		BuildMode::Publish
	};
	let progress = Progress::default();
	let report = project_edit::rebuild(
		EditRequest {
			project: &arguments.project,
			changes: &arguments.changes,
			output: arguments.output.clone(),
			expected_project_sha256: arguments.expect_project_sha256.as_deref(),
			definitions: project_input::definitions(&arguments.define)?,
			mode,
		},
		Default::default(),
		|event| match event {
			BuildEvent::Progress(event) => progress.update(event),
			BuildEvent::MetadataLosses(entries) => {
				report_losses(entries);
			}
		},
	)?;
	Report::from(&report).write(arguments.json)
}

#[derive(Serialize)]
struct Report<'path> {
	schema_version: u16,
	project_sha256: &'path str,
	output: Cow<'path, str>,
	edits: usize,
	entries: usize,
	logical_bytes: u64,
	physical_bytes: Option<u64>,
	metadata_losses: usize,
	published: bool,
	verified: bool,
	trust: &'static str,
	release_stages_deferred: bool,
}

impl<'path> From<&'path EditReport> for Report<'path> {
	fn from(report: &'path EditReport) -> Self {
		let publication = report.publication.as_ref();
		Self {
			schema_version: SCHEMA_VERSION,
			project_sha256: &report.project_sha256,
			output: report.output.to_string_lossy(),
			edits: report.edits,
			entries: report.entries,
			logical_bytes: report.logical_bytes,
			physical_bytes: publication.map(|outcome| outcome.physical_bytes),
			metadata_losses: report.metadata_losses,
			published: publication.is_some(),
			verified: publication.is_some_and(|outcome| outcome.verified),
			trust: UNSIGNED_TRUST,
			release_stages_deferred: report.release_stages_deferred,
		}
	}
}

impl Report<'_> {
	fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |output| {
			writeln!(output, "project_sha256 = {}", self.project_sha256)?;
			writeln!(output, "output = {}", EscapedText(&self.output))?;
			writeln!(output, "edits = {}", self.edits)?;
			writeln!(output, "entries = {}", self.entries)?;
			writeln!(output, "logical_bytes = {}", self.logical_bytes)?;
			if let Some(bytes) = self.physical_bytes {
				writeln!(output, "physical_bytes = {bytes}")?;
			}
			writeln!(output, "metadata_losses = {}", self.metadata_losses)?;
			writeln!(output, "published = {}", self.published)?;
			writeln!(output, "verified = {}", self.verified)?;
			writeln!(output, "trust = {}", self.trust)?;
			writeln!(
				output,
				"release_stages_deferred = {}",
				self.release_stages_deferred
			)
		})
	}
}
