use std::io::{self, Write};

use dmgez_hfsplus_read::analysis::{self, MAX_LARGEST_FILES};
use dmgez_inspector::{AnalysisReport, SelectedWindowId};
use dmgez_udif::reader::{Image, Limits as ImageLimits, ReadAt};
use dmgez_vfs::{Disk, Limits as DiskLimits};
use serde::Serialize;

use crate::{
	CliResult, inspect,
	report_output::write_report,
	volume_input::{BorrowedSource, select},
};

use super::Arguments;

const FILESYSTEM_KEY: &str = "filesystem";
const VOLUME_KEY: &str = "volume";
const ALLOCATION_BLOCK_BYTES_KEY: &str = "allocation_block_bytes";
const ALLOCATION_TOTAL_BLOCKS_KEY: &str = "allocation_total_blocks";
const ALLOCATION_FREE_BLOCKS_KEY: &str = "allocation_free_blocks";
const ALLOCATION_METADATA_BLOCKS_KEY: &str = "allocation_metadata_blocks";
const ALLOCATION_OTHER_BLOCKS_KEY: &str = "allocation_other_allocated_blocks";
const CATALOG_ENTRIES_KEY: &str = "catalog_entries";
const CATALOG_COMPLETE_KEY: &str = "catalog_complete";
const REGULAR_FILES_KEY: &str = "regular_files";
const SIZED_FILES_KEY: &str = "sized_files";
const SCAN_LIMIT_KEY: &str = "catalog_scan_limit";
const UNAVAILABLE_SIZE_KEY: &str = "unavailable_size";
const LARGEST_FILE_KEY: &str = "largest_file";

#[derive(Serialize)]
struct Report {
	#[serde(flatten)]
	container: AnalysisReport,
	volume: SelectedWindowId,
	filesystem: analysis::Report,
}

pub(super) fn run(arguments: &Arguments) -> CliResult {
	let mut limits = analysis::Limits::default();
	if let Some(largest) = arguments.largest {
		let largest = largest.get();
		if largest > MAX_LARGEST_FILES {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				format!("--largest must be at most {MAX_LARGEST_FILES}"),
			)
			.into());
		}
		limits.largest_files = largest;
	}
	let source = inspect::open_source(&arguments.image)?;
	let mut image = Image::open(BorrowedSource(&source), ImageLimits::default())?;
	let container = dmgez_inspector::analyze(&image);
	let mut disk = Disk::discover(&mut image, DiskLimits::default())?;
	let selected = select(&disk, arguments.partition)?;
	let volume = SelectedWindowId::from_volume(selected).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::InvalidInput,
			"unsupported HFS+ analysis window",
		)
	})?;
	let filesystem = analysis::analyze(&mut disk, selected, limits)?;
	source.check_unchanged()?;
	let report = Report {
		container,
		volume,
		filesystem,
	};
	write_report(arguments.json, &report, |output| {
		write_text(output, &report)
	})
}

fn write_text(output: &mut impl Write, report: &Report) -> io::Result<()> {
	inspect::write_analysis(output, &report.container)?;
	writeln!(
		output,
		"{FILESYSTEM_KEY} = {}",
		report.filesystem.format.as_str()
	)?;
	match report.volume {
		SelectedWindowId::WholeDisk => writeln!(output, "{VOLUME_KEY} = whole disk")?,
		SelectedWindowId::Partition { id } => {
			writeln!(
				output,
				"{VOLUME_KEY} = {} partition {}",
				id.scheme.as_str(),
				id.slot
			)?;
		}
		_ => {}
	}
	let allocation = &report.filesystem.allocation;
	writeln!(
		output,
		"{ALLOCATION_BLOCK_BYTES_KEY} = {}",
		allocation.block_size
	)?;
	writeln!(
		output,
		"{ALLOCATION_TOTAL_BLOCKS_KEY} = {}",
		allocation.total_blocks
	)?;
	writeln!(
		output,
		"{ALLOCATION_FREE_BLOCKS_KEY} = {}",
		allocation.free_blocks
	)?;
	writeln!(
		output,
		"{ALLOCATION_METADATA_BLOCKS_KEY} = {}",
		allocation.metadata_blocks
	)?;
	writeln!(
		output,
		"{ALLOCATION_OTHER_BLOCKS_KEY} = {}",
		allocation.other_allocated_blocks
	)?;
	let content = &report.filesystem.content;
	writeln!(output, "{CATALOG_ENTRIES_KEY} = {}", content.entries)?;
	writeln!(output, "{CATALOG_COMPLETE_KEY} = {}", content.complete)?;
	writeln!(output, "{REGULAR_FILES_KEY} = {}", content.regular_files)?;
	writeln!(output, "{SIZED_FILES_KEY} = {}", content.sized_files)?;
	for limit in &content.limits_reached {
		writeln!(output, "{SCAN_LIMIT_KEY} = {}", limit.as_str())?;
	}
	for unavailable in &content.unavailable_sizes {
		writeln!(
			output,
			"{UNAVAILABLE_SIZE_KEY} = {} files ({})",
			unavailable.files,
			unavailable.reason.as_str()
		)?;
	}
	for file in &content.largest_files {
		writeln!(
			output,
			"{LARGEST_FILE_KEY} = {} bytes {}",
			file.logical_bytes,
			file.display_path()
		)?;
	}
	Ok(())
}
