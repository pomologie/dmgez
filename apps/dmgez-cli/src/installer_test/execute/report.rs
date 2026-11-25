use std::{borrow::Cow, fmt::Display, io, io::Write, path::Path, process::ExitCode};

use dmgez_operations::install::InstallerReport;
use serde::Serialize;

use crate::{
	CliResult, REPORT_ERROR_EXIT_CODE,
	report_output::{EscapedBytes, EscapedText, write_report},
	volume_commands::{APFS_FILESYSTEM_VALUE, Filesystem, HFSPLUS_FILESYSTEM_VALUE},
};

use super::Arguments;

const SCHEMA_VERSION: u16 = 1;
const PROFILE: &str = "installer";

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
	Passed,
	Failed,
	Unavailable,
	Blocked,
	Skipped,
	Error,
}

impl Status {
	fn as_str(self) -> &'static str {
		match self {
			Self::Passed => "passed",
			Self::Failed => "failed",
			Self::Unavailable => "unavailable",
			Self::Blocked => "blocked",
			Self::Skipped => "skipped",
			Self::Error => "error",
		}
	}
}

#[derive(Serialize)]
pub(super) struct Check<T = ()> {
	pub(super) status: Status,
	#[serde(skip_serializing_if = "Option::is_none")]
	detail: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	evidence: Option<T>,
}

impl<T> Check<T> {
	pub(super) const fn new(status: Status) -> Self {
		Self {
			status,
			detail: None,
			evidence: None,
		}
	}

	pub(super) fn detail(status: Status, detail: impl Display) -> Self {
		Self {
			detail: Some(detail.to_string()),
			..Self::new(status)
		}
	}

	pub(super) fn evidence(status: Status, evidence: T) -> Self {
		Self {
			evidence: Some(evidence),
			..Self::new(status)
		}
	}

	const fn requested(required: bool) -> Self {
		Self::new(if required {
			Status::Blocked
		} else {
			Status::Skipped
		})
	}

	fn write(&self, output: &mut impl Write, label: &str) -> io::Result<()> {
		writeln!(output, "{label} = {}", self.status.as_str())?;
		if let Some(detail) = &self.detail {
			writeln!(output, "{label}_detail = {}", EscapedText(detail))?;
		}
		Ok(())
	}
}

#[derive(Serialize)]
pub(super) struct Checksums {
	pub(super) data_fork_verified: bool,
	pub(super) master_verified: bool,
	pub(super) tables: usize,
	pub(super) verified_tables: usize,
}

#[derive(Serialize)]
pub(super) struct Report<'path> {
	schema_version: u16,
	profile: &'static str,
	artifact: Cow<'path, str>,
	filesystem: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	partition: Option<u32>,
	#[cfg(feature = "apfs-read")]
	#[serde(skip_serializing_if = "Option::is_none")]
	pub(super) apfs_volume: Option<crate::volume_commands::VolumeIdentity>,
	pub(super) source: Check,
	pub(super) container: Check<Checksums>,
	pub(super) layout: Check<InstallerReport>,
	pub(super) signature: Check<&'static str>,
	pub(super) ticket: Check<&'static str>,
	pub(super) native_assessment: Check<&'static str>,
}

impl<'path> Report<'path> {
	pub(super) fn new(arguments: &Arguments, path: &'path Path) -> Self {
		Self {
			schema_version: SCHEMA_VERSION,
			profile: PROFILE,
			artifact: path.to_string_lossy(),
			filesystem: match arguments.selection.filesystem() {
				Filesystem::Hfsplus => HFSPLUS_FILESYSTEM_VALUE,
				Filesystem::Apfs => APFS_FILESYSTEM_VALUE,
			},
			partition: arguments.partition.map(std::num::NonZeroU32::get),
			#[cfg(feature = "apfs-read")]
			apfs_volume: None,
			source: Check::new(Status::Blocked),
			container: Check::new(Status::Blocked),
			layout: Check::new(Status::Blocked),
			signature: Check::requested(arguments.require_signature),
			ticket: Check::requested(arguments.require_ticket),
			native_assessment: Check::requested(arguments.require_native_assessment),
		}
	}

	pub(super) fn exit_code(&self) -> ExitCode {
		let statuses = [
			self.source.status,
			self.container.status,
			self.layout.status,
			self.signature.status,
			self.ticket.status,
			self.native_assessment.status,
		];
		if statuses
			.iter()
			.any(|status| matches!(status, Status::Unavailable | Status::Error))
		{
			ExitCode::from(REPORT_ERROR_EXIT_CODE)
		} else if statuses.contains(&Status::Failed) {
			ExitCode::FAILURE
		} else if statuses.contains(&Status::Blocked) {
			ExitCode::from(REPORT_ERROR_EXIT_CODE)
		} else {
			ExitCode::SUCCESS
		}
	}

	pub(super) fn write(&self, json: bool) -> CliResult {
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
			self.source.write(output, "source")?;
			self.container.write(output, "container")?;
			if let Some(checksums) = &self.container.evidence {
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
			self.layout.write(output, "layout")?;
			if let Some(layout) = &self.layout.evidence {
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
				("signature", &self.signature),
				("ticket", &self.ticket),
				("native_assessment", &self.native_assessment),
			] {
				check.write(output, label)?;
				if let Some(evidence) = check.evidence {
					writeln!(output, "{label}_evidence = {evidence}")?;
				}
			}
			Ok(())
		})
	}
}
