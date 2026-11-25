use std::{
	io::{self, Write},
	path::Path,
	process::ExitCode,
};

use dmgez_lint::installer::{Limits, Report, RuleId, Suppression};
use dmgez_runtime::lint::installer::lint_project;
use serde::Serialize;

use super::{LINT_FINDINGS_EXIT_CODE, suppression_parts};
use crate::{
	CliResult, INSTALLER_PROFILE_NAME,
	report_output::{EscapedText, write_report},
};

#[derive(Serialize)]
struct InstallerReport<'input> {
	profile: &'static str,
	#[serde(flatten)]
	installer: Report<'input>,
}

pub(crate) fn run(
	path: &Path,
	folder: Option<&str>,
	suppressions: &[String],
	json: bool,
) -> CliResult<ExitCode> {
	if folder.is_some() {
		return Err("--folder is only supported by the layout lint profile".into());
	}
	let suppressions = suppressions
		.iter()
		.map(|argument| parse_suppression(argument))
		.collect::<CliResult<Vec<_>>>()?;
	let installer = lint_project(path, Limits::default(), &suppressions, || false)?;
	let report = InstallerReport {
		profile: INSTALLER_PROFILE_NAME,
		installer,
	};
	write_report(json, &report, |output| write_text(output, &report))?;
	Ok(if report.installer.has_warnings() {
		ExitCode::from(LINT_FINDINGS_EXIT_CODE)
	} else {
		ExitCode::SUCCESS
	})
}

fn parse_suppression(argument: &str) -> CliResult<Suppression<'_>> {
	let (rule, reason) = suppression_parts(argument)?;
	Ok(Suppression::new(rule.parse::<RuleId>()?, reason)?)
}

fn write_text(output: &mut impl Write, report: &InstallerReport<'_>) -> io::Result<()> {
	writeln!(output, "profile = {}", report.profile)?;
	writeln!(
		output,
		"diagnostics = {}",
		report.installer.diagnostics.len()
	)?;
	for diagnostic in &report.installer.diagnostics {
		write!(
			output,
			"{} {} \"{}\"",
			diagnostic.severity.as_str(),
			diagnostic.rule.as_str(),
			EscapedText(diagnostic.evidence.destination()),
		)?;
		if let Some(reason) = diagnostic.suppression_reason {
			write!(output, " (suppressed \"{}\")", EscapedText(reason))?;
		}
		writeln!(output)?;
	}
	Ok(())
}
