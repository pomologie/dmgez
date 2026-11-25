use std::io::{self, Write};

use dmgez_inspector::{
	SelectedWindowId,
	volume_queries::{ListReport, StatReport, TreeReport},
};
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedBytes, EscapedText, write_report},
};

#[cfg(feature = "apfs-read")]
use super::apfs::VolumeIdentity;

const NEXT_OFFSET_KEY: &str = "next_offset";
const TRUNCATED_KEY: &str = "truncated";
const PATH_KEY: &str = "path";
const VOLUME_KEY: &str = "volume";
const NAME_KEY: &str = "name";
const KIND_KEY: &str = "kind";
const NATIVE_ID_KEY: &str = "native_id";
const MODE_KEY: &str = "mode";
const UID_KEY: &str = "uid";
const GID_KEY: &str = "gid";
const HARD_LINK_COUNT_KEY: &str = "hard_link_count";
const HARD_LINK_STORAGE_ID_KEY: &str = "hard_link_storage_id";
const LINK_TARGET_KEY: &str = "link_target";
const TREE_INDENT_WIDTH: usize = 2;

pub(super) enum Report {
	List(ListReport),
	Tree(TreeReport),
	Stat(StatReport),
	Cat(io::StdoutLock<'static>),
}

pub(super) enum ReadVolume {
	#[cfg(feature = "hfsplus-read")]
	Hfsplus,
	#[cfg(feature = "apfs-read")]
	Apfs(VolumeIdentity),
}

impl Report {
	pub(super) fn write(self, json: bool, volume: ReadVolume) -> CliResult {
		match self {
			Self::List(report) => {
				write_browse_report(json, &report, volume, |output| write_list(output, &report))
			}
			Self::Tree(report) => {
				write_browse_report(json, &report, volume, |output| write_tree(output, &report))
			}
			Self::Stat(report) => {
				write_browse_report(json, &report, volume, |output| write_stat(output, &report))
			}
			Self::Cat(mut output) => Ok(output.flush()?),
		}
	}
}

fn write_browse_report(
	json: bool,
	report: &impl Serialize,
	volume: ReadVolume,
	text: impl FnOnce(&mut io::StdoutLock<'_>) -> io::Result<()>,
) -> CliResult {
	match volume {
		#[cfg(feature = "hfsplus-read")]
		ReadVolume::Hfsplus => write_report(json, report, text),
		#[cfg(feature = "apfs-read")]
		ReadVolume::Apfs(apfs_volume) => write_report(
			json,
			&ApfsBrowseReport {
				report,
				apfs_volume,
			},
			|output| {
				apfs_volume.write(output)?;
				text(output)
			},
		),
	}
}

#[cfg(feature = "apfs-read")]
#[derive(Serialize)]
struct ApfsBrowseReport<'report, T> {
	#[serde(flatten)]
	report: &'report T,
	apfs_volume: VolumeIdentity,
}

fn write_list(output: &mut impl Write, report: &ListReport) -> io::Result<()> {
	for entry in &report.entries {
		writeln!(output, "{}\t{}", entry.kind.as_str(), entry.name)?;
	}
	if let Some(offset) = report.next_offset {
		writeln!(output, "{NEXT_OFFSET_KEY} = {offset}")?;
	}
	if report.truncated {
		writeln!(output, "{TRUNCATED_KEY} = true")?;
	}
	Ok(())
}

fn write_tree(output: &mut impl Write, report: &TreeReport) -> io::Result<()> {
	writeln!(output, "{}", EscapedText(&report.path))?;
	for entry in &report.entries {
		let width = usize::from(entry.depth) * TREE_INDENT_WIDTH;
		writeln!(
			output,
			"{:width$}{}\t{}",
			"",
			entry.kind.as_str(),
			entry.name
		)?;
	}
	if report.truncated {
		writeln!(output, "{TRUNCATED_KEY} = true")?;
	}
	Ok(())
}

fn write_stat(output: &mut impl Write, report: &StatReport) -> io::Result<()> {
	writeln!(output, "{PATH_KEY} = {}", EscapedText(&report.path))?;
	write_window(output, report.volume)?;
	if let Some(name) = &report.name {
		writeln!(output, "{NAME_KEY} = {name}")?;
	}
	writeln!(output, "{KIND_KEY} = {}", report.kind.as_str())?;
	if let Some(identity) = report.native_id {
		writeln!(output, "{NATIVE_ID_KEY} = {}", identity.0)?;
	}
	writeln!(output, "{MODE_KEY} = {:o}", report.mode)?;
	writeln!(output, "{UID_KEY} = {}", report.uid)?;
	writeln!(output, "{GID_KEY} = {}", report.gid)?;
	if let Some(count) = report.hard_link_count {
		writeln!(output, "{HARD_LINK_COUNT_KEY} = {count}")?;
	}
	if let Some(identity) = report.hard_link_storage_id {
		writeln!(output, "{HARD_LINK_STORAGE_ID_KEY} = {}", identity.0)?;
	}
	if let Some(target) = &report.link_target {
		writeln!(output, "{LINK_TARGET_KEY} = {}", EscapedBytes(target))?;
	}
	Ok(())
}

pub(super) fn write_window(output: &mut impl Write, window: SelectedWindowId) -> io::Result<()> {
	match window {
		SelectedWindowId::WholeDisk => writeln!(output, "{VOLUME_KEY} = whole disk"),
		SelectedWindowId::Partition { id } => writeln!(
			output,
			"{VOLUME_KEY} = {} partition {}",
			id.scheme.as_str(),
			id.slot,
		),
		_ => Ok(()),
	}
}
