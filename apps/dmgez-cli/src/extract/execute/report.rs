use std::{borrow::Cow, error::Error, io::Write, os::unix::ffi::OsStrExt, path::Path};

use dmgez_inspector::SCHEMA_VERSION;
use dmgez_operations::extract::{PlanReport, Progress};
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedBytes, EscapedText, write_report},
};

const OPERATION: &str = "extract";
const STATUS_KEY: &str = "status";
const SOURCE_KEY: &str = "source_path";
const DESTINATION_KEY: &str = "destination";
const SELECTION_KEY: &str = "selection";
const PLAN_FILES_KEY: &str = "planned_files";
const PLAN_DIRECTORIES_KEY: &str = "planned_directories";
const PLAN_BYTES_KEY: &str = "planned_bytes";
const MATERIALIZED_LINKS_KEY: &str = "hard_links_materialized";
const OMISSIONS_KEY: &str = "omitted";
const FILES_PUBLISHED_KEY: &str = "files_published";
const DIRECTORIES_CREATED_KEY: &str = "directories_created";
const DIRECTORY_CREATION_UNCONFIRMED_KEY: &str = "directory_creation_unconfirmed";
const BYTES_PUBLISHED_KEY: &str = "bytes_published";
const MAX_REPORTED_CAUSES: usize = 8;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
	Planned,
	Complete,
	Failed,
	Partial,
}

impl Status {
	const fn as_str(self) -> &'static str {
		match self {
			Self::Planned => "planned",
			Self::Complete => "complete",
			Self::Failed => "failed",
			Self::Partial => "partial",
		}
	}
}

#[derive(Serialize)]
pub(super) struct Report<'a> {
	schema_version: u32,
	operation: &'static str,
	status: Status,
	source_path: &'a str,
	destination: Cow<'a, str>,
	#[serde(skip_serializing_if = "Option::is_none")]
	destination_bytes: Option<&'a [u8]>,
	pub(super) plan: Option<PlanReport>,
	pub(super) progress: Option<Progress>,
	#[serde(skip_serializing_if = "Option::is_none")]
	error: Option<String>,
	#[serde(skip_serializing_if = "Vec::is_empty")]
	causes: Vec<String>,
}

impl<'a> Report<'a> {
	pub(super) fn new(source_path: &'a str, destination: &'a Path) -> Self {
		Self {
			schema_version: SCHEMA_VERSION,
			operation: OPERATION,
			status: Status::Failed,
			source_path,
			destination: destination.to_string_lossy(),
			destination_bytes: destination
				.to_str()
				.is_none()
				.then_some(destination.as_os_str().as_bytes()),
			plan: None,
			progress: None,
			error: None,
			causes: Vec::new(),
		}
	}

	pub(super) fn finish(&mut self, dry_run: bool) {
		self.status = if dry_run {
			Status::Planned
		} else {
			Status::Complete
		};
	}

	pub(super) fn fail(&mut self, error: &dyn Error) {
		let partial = self.progress.is_some_and(|progress| {
			progress.directories_created != 0
				|| progress.files_published != 0
				|| progress.directory_creation_unconfirmed
		});
		self.status = if partial {
			Status::Partial
		} else {
			Status::Failed
		};
		self.error = Some(error.to_string());
		let mut cause = error.source();
		for _ in 0..MAX_REPORTED_CAUSES {
			let Some(current) = cause else { break };
			self.causes.push(current.to_string());
			cause = current.source();
		}
	}

	pub(super) fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |output| {
			writeln!(output, "{STATUS_KEY} = {}", self.status.as_str())?;
			writeln!(output, "{SOURCE_KEY} = {}", EscapedText(self.source_path))?;
			writeln!(
				output,
				"{DESTINATION_KEY} = {}",
				EscapedBytes(
					self.destination_bytes
						.unwrap_or(self.destination.as_bytes())
				)
			)?;
			if let Some(plan) = self.plan {
				write!(output, "{SELECTION_KEY} = ")?;
				serde_json::to_writer(&mut *output, &plan.selection)?;
				writeln!(output)?;
				writeln!(output, "{PLAN_FILES_KEY} = {}", plan.files)?;
				writeln!(output, "{PLAN_DIRECTORIES_KEY} = {}", plan.directories)?;
				writeln!(output, "{PLAN_BYTES_KEY} = {}", plan.bytes)?;
				writeln!(
					output,
					"{MATERIALIZED_LINKS_KEY} = {}",
					plan.hard_links_materialized
				)?;
				write!(output, "{OMISSIONS_KEY} = ")?;
				serde_json::to_writer(&mut *output, plan.omitted)?;
				writeln!(output)?;
			}
			if let Some(progress) = self.progress {
				writeln!(
					output,
					"{FILES_PUBLISHED_KEY} = {}",
					progress.files_published
				)?;
				writeln!(
					output,
					"{DIRECTORIES_CREATED_KEY} = {}",
					progress.directories_created
				)?;
				writeln!(
					output,
					"{BYTES_PUBLISHED_KEY} = {}",
					progress.bytes_published
				)?;
				if progress.directory_creation_unconfirmed {
					writeln!(output, "{DIRECTORY_CREATION_UNCONFIRMED_KEY} = true")?;
				}
			}
			Ok(())
		})
	}
}

#[cfg(test)]
mod tests;
