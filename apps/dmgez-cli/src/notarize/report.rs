use std::io::{self, Write};

use dmgez_signing::notarization::Report;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

pub(super) fn write(report: &Report, json: bool) -> CliResult {
	write_report(json, report, |output| write_text(output, report))
}

fn write_text(output: &mut impl Write, report: &Report) -> io::Result<()> {
	writeln!(output, "schema_version = {}", report.schema_version)?;
	writeln!(output, "backend = {}", report.backend.as_str())?;
	writeln!(output, "status = {}", report.status.as_str())?;
	if let Some(id) = &report.submission_id {
		writeln!(output, "submission_id = {id}")?;
	}
	if let Some(sha256) = &report.artifact_sha256 {
		writeln!(output, "artifact_sha256 = \"{}\"", EscapedText(sha256))?;
	}
	if let Some(bytes) = report.artifact_bytes {
		writeln!(output, "artifact_bytes = {bytes}")?;
	}
	writeln!(output, "digest_verified = {}", report.digest_verified)?;
	writeln!(output, "created_at = {}", report.created_at)?;
	writeln!(output, "updated_at = {}", report.updated_at)?;
	if let Some(issue) = &report.issue {
		writeln!(output, "issue = {}", issue.as_str())?;
	}
	if let Some(log) = &report.log {
		writeln!(output, "log_errors = {}", log.errors)?;
		writeln!(output, "log_warnings = {}", log.warnings)?;
	}
	Ok(())
}
