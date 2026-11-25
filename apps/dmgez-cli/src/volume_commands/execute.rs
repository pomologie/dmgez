use std::io::{self, Write};

use dmgez_inspector::{
	SelectedWindowId,
	volume_queries::{self, DEFAULT_LIST_LIMIT, DEFAULT_TREE_DEPTH, DEFAULT_TREE_LIMIT},
};
use dmgez_vfs::volume::ForkKind;

use crate::{
	CliResult,
	report_output::{EscapedBytes, EscapedText, write_report},
	volume_input::with_hfs_view,
};

use super::{CatArguments, Fork, ListArguments, StatArguments, TreeArguments};

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

pub(crate) fn list(arguments: &ListArguments) -> CliResult {
	let input = &arguments.input;
	let report = with_hfs_view(&input.image, input.raw, input.partition, |view| {
		Ok(volume_queries::list(
			view,
			&arguments.path,
			arguments.offset,
			arguments.limit.unwrap_or(DEFAULT_LIST_LIMIT),
		)?)
	})?;
	write_report(arguments.json, &report, |output| {
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
	})
}

pub(crate) fn tree(arguments: &TreeArguments) -> CliResult {
	let input = &arguments.input;
	let report = with_hfs_view(&input.image, input.raw, input.partition, |view| {
		Ok(volume_queries::tree(
			view,
			&arguments.path,
			arguments.depth.unwrap_or(DEFAULT_TREE_DEPTH),
			arguments.limit.unwrap_or(DEFAULT_TREE_LIMIT),
		)?)
	})?;
	write_report(arguments.json, &report, |output| {
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
	})
}

pub(crate) fn stat(arguments: &StatArguments) -> CliResult {
	let input = &arguments.input;
	let report = with_hfs_view(&input.image, input.raw, input.partition, |view| {
		Ok(volume_queries::stat(view, &arguments.path)?)
	})?;
	write_report(arguments.json, &report, |output| {
		writeln!(output, "{PATH_KEY} = {}", EscapedText(&report.path))?;
		match report.volume {
			SelectedWindowId::WholeDisk => writeln!(output, "{VOLUME_KEY} = whole disk")?,
			SelectedWindowId::Partition { id } => writeln!(
				output,
				"{VOLUME_KEY} = {} partition {}",
				id.scheme.as_str(),
				id.slot,
			)?,
			_ => {}
		}
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
	})
}

pub(crate) fn cat(arguments: &CatArguments) -> CliResult {
	let input = &arguments.input;
	let fork = match arguments.fork {
		Fork::Data => ForkKind::Data,
		Fork::Resource => ForkKind::Resource,
	};
	let mut output = io::stdout().lock();
	with_hfs_view(&input.image, input.raw, input.partition, |view| {
		volume_queries::cat(view, &arguments.path, fork, &mut output)?;
		Ok(())
	})?;
	output.flush()?;
	Ok(())
}
