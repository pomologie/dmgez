use std::io::{self, Write};

#[cfg(feature = "signing-native")]
use dmgez_signing::AppSigningReport;
use dmgez_signing::{DiskSigningReport, Report};

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

pub(super) fn write(report: &Report, json: bool) -> CliResult {
	write_report(json, report, |output| write_text(output, report))
}

pub(super) fn write_disk_signing(report: &DiskSigningReport, json: bool) -> CliResult {
	write_report(json, report, |output| {
		writeln!(output, "schema_version = {}", report.schema_version)?;
		writeln!(output, "backend = {}", report.backend.as_str())?;
		writeln!(output, "mode = {}", report.mode.as_str())?;
		writeln!(output, "assessment = {}", report.assessment.as_str())?;
		writeln!(output, "source = \"{}\"", EscapedText(&report.source))?;
		writeln!(output, "output = \"{}\"", EscapedText(&report.output))?;
		if let Some(identity) = &report.identity {
			writeln!(output, "identity = \"{}\"", EscapedText(identity))?;
		}
		writeln!(
			output,
			"identifier = \"{}\"",
			EscapedText(&report.identifier)
		)?;
		writeln!(output, "unsigned_sha256 = {}", report.unsigned_sha256)?;
		writeln!(output, "signed_sha256 = {}", report.signed_sha256)
	})
}

#[cfg(feature = "signing-native")]
pub(super) fn write_app_signing(report: &AppSigningReport, json: bool) -> CliResult {
	write_report(json, report, |output| {
		writeln!(output, "schema_version = {}", report.schema_version)?;
		writeln!(output, "backend = {}", report.backend.as_str())?;
		writeln!(output, "assessment = {}", report.assessment.as_str())?;
		writeln!(output, "source = \"{}\"", EscapedText(&report.source))?;
		writeln!(output, "output = \"{}\"", EscapedText(&report.output))?;
		writeln!(output, "identity = \"{}\"", EscapedText(&report.identity))?;
		for item in &report.items {
			writeln!(
				output,
				"item = \"{}\" hardened_runtime = {} entitlements = {}",
				EscapedText(&item.item),
				item.hardened_runtime,
				item.entitlements,
			)?;
		}
		Ok(())
	})
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
