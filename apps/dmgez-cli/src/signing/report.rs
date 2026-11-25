use std::io::{self, Write};

use dmgez_signing::Report;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

pub(super) fn write(report: &Report, json: bool) -> CliResult {
	write_report(json, report, |output| write_text(output, report))
}

fn write_text(output: &mut impl Write, report: &Report) -> io::Result<()> {
	writeln!(output, "schema_version = {}", report.schema_version)?;
	writeln!(output, "operation = {}", report.operation.as_str())?;
	writeln!(
		output,
		"requested_backend = {}",
		report.requested_backend.as_str()
	)?;
	if let Some(backend) = &report.backend {
		writeln!(output, "backend = {}", backend.as_str())?;
	}
	writeln!(output, "status = {}", report.status.as_str())?;
	writeln!(output, "assessment = {}", report.assessment.as_str())?;
	writeln!(output, "artifact = \"{}\"", EscapedText(&report.artifact))?;
	if let Some(kind) = &report.kind {
		writeln!(output, "kind = {}", kind.as_str())?;
	}
	if let Some(diagnostics) = &report.diagnostics {
		writeln!(output, "diagnostics = \"{}\"", EscapedText(diagnostics))?;
	}
	Ok(())
}
