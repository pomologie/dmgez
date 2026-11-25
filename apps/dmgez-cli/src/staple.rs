use std::path::PathBuf;

use clap::Args;

use crate::CliResult;

#[derive(Args)]
pub(super) struct Arguments {
	/// Original signed disk image accepted for notarization.
	#[arg(value_name = "IMAGE")]
	image: PathBuf,
	/// Private submission record for the accepted image.
	#[arg(long, value_name = "PATH")]
	record: PathBuf,
	/// New path for the validated stapled image.
	#[arg(long, value_name = "PATH")]
	output: PathBuf,
	/// Write a versioned JSON report.
	#[arg(long)]
	json: bool,
}

#[cfg(not(feature = "notarization-native"))]
pub(super) fn run(_arguments: &Arguments) -> CliResult {
	Err(format!("feature not compiled: {}", crate::notarize::NATIVE_FEATURE).into())
}

#[cfg(all(feature = "notarization-native", not(target_os = "macos")))]
pub(super) fn run(_arguments: &Arguments) -> CliResult {
	Err("native stapling is unavailable on this platform".into())
}

#[cfg(all(feature = "notarization-native", target_os = "macos"))]
pub(super) fn run(arguments: &Arguments) -> CliResult {
	use std::io::Write;

	use crate::report_output::{EscapedText, write_report};

	let report = dmgez_signing::notarization::staple(
		&arguments.image,
		&arguments.record,
		&arguments.output,
	)?;
	write_report(arguments.json, &report, |output| {
		writeln!(output, "schema_version = {}", report.schema_version)?;
		writeln!(output, "backend = {}", report.backend.as_str())?;
		writeln!(output, "submission_id = {}", report.submission_id)?;
		writeln!(output, "source = \"{}\"", EscapedText(&report.source))?;
		writeln!(output, "output = \"{}\"", EscapedText(&report.output))?;
		writeln!(
			output,
			"input_sha256 = \"{}\"",
			EscapedText(&report.input_sha256)
		)?;
		writeln!(
			output,
			"output_sha256 = \"{}\"",
			EscapedText(&report.output_sha256)
		)?;
		writeln!(output, "ticket_validated = {}", report.ticket_validated)?;
		writeln!(output, "assessment = {}", report.assessment.as_str())
	})
}
