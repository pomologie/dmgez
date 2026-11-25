use std::{borrow::Cow, io, io::Write, process::ExitCode};

use dmgez_runtime::installer_test::{Check, Checks, Outcome, Report as TestReport};
use serde::Serialize;

use crate::{
	CliResult, REPORT_ERROR_EXIT_CODE,
	report_output::{EscapedBytes, EscapedText, write_report},
	volume_commands::{APFS_FILESYSTEM_VALUE, Filesystem, HFSPLUS_FILESYSTEM_VALUE},
};

use super::Arguments;

const SCHEMA_VERSION: u16 = 1;
const PROFILE: &str = "installer";

#[derive(Serialize)]
struct Report<'report> {
	schema_version: u16,
	profile: &'static str,
	artifact: Cow<'report, str>,
	filesystem: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	partition: Option<u32>,
	#[cfg(feature = "apfs-read")]
	#[serde(skip_serializing_if = "Option::is_none")]
	apfs_volume: Option<crate::volume_commands::VolumeIdentity>,
	#[serde(flatten)]
	checks: &'report Checks,
}

pub(super) fn write(arguments: &Arguments, observed: &TestReport) -> CliResult<ExitCode> {
	let report = Report {
		schema_version: SCHEMA_VERSION,
		profile: PROFILE,
		artifact: observed.artifact.to_string_lossy(),
		filesystem: match arguments.selection.filesystem() {
			Filesystem::Hfsplus => HFSPLUS_FILESYSTEM_VALUE,
			Filesystem::Apfs => APFS_FILESYSTEM_VALUE,
		},
		partition: arguments.partition.map(std::num::NonZeroU32::get),
		#[cfg(feature = "apfs-read")]
		apfs_volume: observed
			.apfs_volume
			.as_ref()
			.map(crate::volume_commands::VolumeIdentity::from),
		checks: &observed.checks,
	};
	report.write(arguments.json)?;
	Ok(match observed.checks.outcome() {
		Outcome::Passed => ExitCode::SUCCESS,
		Outcome::Failed => ExitCode::FAILURE,
		Outcome::Incomplete => ExitCode::from(REPORT_ERROR_EXIT_CODE),
	})
}

fn write_check<T>(output: &mut impl Write, label: &str, check: &Check<T>) -> io::Result<()> {
	writeln!(output, "{label} = {}", check.status.as_str())?;
	if let Some(detail) = &check.detail {
		writeln!(output, "{label}_detail = {}", EscapedText(detail))?;
	}
	Ok(())
}

impl Report<'_> {
	fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |output| {
			writeln!(output, "profile = {}", self.profile)?;
			writeln!(output, "artifact = {}", EscapedText(&self.artifact))?;
			writeln!(output, "filesystem = {}", self.filesystem)?;
			if let Some(partition) = self.partition {
				writeln!(output, "partition = {partition}")?;
			}
			#[cfg(feature = "apfs-read")]
			if let Some(volume) = &self.apfs_volume {
				volume.write(output)?;
			}
			write_check(output, "source", &self.checks.source)?;
			write_check(output, "container", &self.checks.container)?;
			if let Some(checksums) = &self.checks.container.evidence {
				writeln!(
					output,
					"data_fork_crc_verified = {}",
					checksums.data_fork_verified
				)?;
				writeln!(
					output,
					"master_crc_verified = {}",
					checksums.master_verified
				)?;
				writeln!(
					output,
					"table_crcs_verified = {}/{}",
					checksums.verified_tables, checksums.tables
				)?;
			}
			write_check(output, "layout", &self.checks.layout)?;
			if let Some(layout) = &self.checks.layout.evidence {
				writeln!(output, "applications = {}", layout.applications)?;
				for entry in &layout.diagnostics {
					write!(
						output,
						"{} at {}",
						entry.rule.as_str(),
						EscapedText(&entry.path)
					)?;
					if let Some(target) = &entry.target {
						write!(output, " -> {}", EscapedBytes(target))?;
					}
					writeln!(output)?;
				}
			}
			for (label, check) in [
				("signature", &self.checks.signature),
				("ticket", &self.checks.ticket),
				("native_assessment", &self.checks.native_assessment),
			] {
				write_check(output, label, check)?;
				if let Some(evidence) = check.evidence {
					writeln!(output, "{label}_evidence = {evidence}")?;
				}
			}
			Ok(())
		})
	}
}
