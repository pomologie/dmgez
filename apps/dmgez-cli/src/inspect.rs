use std::{
	error::Error,
	fmt,
	fs::File,
	io::{self, Write},
	path::Path,
};

use dmgez_core::output::publish_new;
use dmgez_inspector::{
	CheckStatus, ChecksumDeclaration, DiskLimits, DiskReport, MAX_RUN_PAGE_SIZE, RunWindow,
	VerificationMode, VerificationOutcome, VerificationReport,
};
use dmgez_udif::reader::{FileSource, Image, Limits};
use serde::Serialize;

use crate::{CliResult, VerificationFlags, report_field};

const FORMAT_KEY: &str = "format";
const PHYSICAL_BYTES_KEY: &str = "physical_bytes";
const LOGICAL_BYTES_KEY: &str = "logical_bytes";
const VARIANT_KEY: &str = "variant";
const BLOCK_MAP_COUNT_KEY: &str = "block_maps";
const DATA_FORK_CHECKSUM_KEY: &str = "data_fork_checksum";
const MASTER_CHECKSUM_KEY: &str = "master_checksum";
const UNSUPPORTED_RUNS_KEY: &str = "unsupported_runs";
const UNAVAILABLE_DECODERS_KEY: &str = "runs_with_unavailable_decoders";
const RUN_KEY: &str = "runs";
const NEXT_RUN_OFFSET_KEY: &str = "next_run_offset";
const DISK_KEY: &str = "disk";
const DISK_BLOCK_BYTES_KEY: &str = "disk_block_bytes";
const PARTITION_KEY: &str = "partition";
const PARTITIONLESS: &str = "partitionless";
const UNRESOLVED: &str = "unresolved";
const VERIFICATION_KEY: &str = "verification";
const DATA_FORK_STATUS_KEY: &str = "data_fork";
const BLOCK_MAP_STATUS_KEY: &str = "block_map";
const MASTER_STATUS_KEY: &str = "master";
const STRUCTURE_STATUS_KEY: &str = "structure";
const PAYLOAD_STATUS_KEY: &str = "payload";
const FILESYSTEM_STATUS_KEY: &str = "filesystem";
const UDIF_V4: &str = "UDIF v4";
const CRC32: &str = "crc32";
const UNKNOWN: &str = "unknown";
const PARTIAL: &str = "partial";

fn open_source(path: &Path) -> Result<FileSource, dmgez_udif::Error> {
	FileSource::new(File::open(path)?)
}

pub(crate) fn show(
	path: &Path,
	json: bool,
	partitions: bool,
	offset: usize,
	limit: Option<usize>,
) -> CliResult {
	let window = match limit {
		Some(limit) => Some(RunWindow::new(offset, limit).ok_or_else(|| {
			io::Error::new(
				io::ErrorKind::InvalidInput,
				format!("run limit must be between 1 and {MAX_RUN_PAGE_SIZE}"),
			)
		})?),
		None => None,
	};
	let mut image = Image::open(open_source(path)?, Limits::default())?;
	let report = if partitions {
		dmgez_inspector::inspect_disk(&mut image, window, DiskLimits::default())
	} else {
		dmgez_inspector::inspect(&image, window)
	};
	if json {
		return print_json(&report);
	}
	let container = report.container;
	report_field(FORMAT_KEY, UDIF_V4);
	report_field(PHYSICAL_BYTES_KEY, container.physical_bytes);
	report_field(LOGICAL_BYTES_KEY, container.logical_bytes);
	report_field(VARIANT_KEY, container.variant);
	report_field(BLOCK_MAP_COUNT_KEY, container.block_map_count);
	report_field(
		DATA_FORK_CHECKSUM_KEY,
		checksum(container.data_fork_checksum),
	);
	report_field(MASTER_CHECKSUM_KEY, checksum(container.master_checksum));
	report_field(UNSUPPORTED_RUNS_KEY, container.runs.unsupported);
	report_field(UNAVAILABLE_DECODERS_KEY, container.runs.decode_unavailable);
	report_field(VERIFICATION_KEY, report.coverage.payload.as_str());
	if let Some(page) = report.run_page {
		for (index, run) in (page.offset..).zip(page.runs) {
			report_field(
				format_args!("{RUN_KEY}[{index}]"),
				format_args!(
					"{:?}, logical byte {} length {}, stored byte {} length {}",
					run.encoding,
					run.logical.offset_bytes,
					run.logical.length_bytes,
					run.physical.offset_bytes,
					run.physical.length_bytes,
				),
			);
		}
		if let Some(offset) = page.next_offset {
			report_field(NEXT_RUN_OFFSET_KEY, offset);
		}
	}
	if let Some(disk) = &report.disk {
		show_disk(disk);
	}
	Ok(())
}

fn show_disk(disk: &DiskReport) {
	match disk {
		DiskReport::Partitioned {
			scheme,
			logical_block_bytes,
			partitions,
		} => {
			report_field(DISK_KEY, scheme.as_str());
			report_field(DISK_BLOCK_BYTES_KEY, logical_block_bytes);
			for partition in partitions {
				let slot = partition.id.slot;
				report_field(
					format_args!("{PARTITION_KEY}[{slot}]"),
					format_args!(
						"{:?}, logical byte {} length {}, {:?}",
						partition.role,
						partition.logical.offset_bytes,
						partition.logical.length_bytes,
						partition.details
					),
				);
			}
		}
		DiskReport::Partitionless {
			filesystem,
			logical,
		} => {
			report_field(
				DISK_KEY,
				format_args!(
					"{PARTITIONLESS} {filesystem:?}, logical byte {} length {}",
					logical.offset_bytes, logical.length_bytes
				),
			);
		}
		DiskReport::Unresolved { diagnostic } => {
			report_field(
				DISK_KEY,
				format_args!("{UNRESOLVED} {:?}", diagnostic.category),
			);
		}
		_ => report_field(DISK_KEY, UNKNOWN),
	}
}

pub(crate) fn verify(path: &Path, json: bool, scope: VerificationFlags) -> CliResult {
	let mode = match scope {
		VerificationFlags { quick: true, .. } => VerificationMode::Quick,
		VerificationFlags { full: true, .. } => VerificationMode::Full,
		_ => VerificationMode::Container,
	};
	let report = dmgez_inspector::verify(open_source(path)?, Limits::default(), mode);
	if json {
		print_json(&report)?;
	} else {
		if let Some(checks) = &report.checksums {
			report_field(DATA_FORK_STATUS_KEY, checks.data_fork.as_str());
			for (index, check) in checks.tables.iter().enumerate() {
				report_field(
					format_args!("{BLOCK_MAP_STATUS_KEY}[{index}]"),
					check.as_str(),
				);
			}
			report_field(MASTER_STATUS_KEY, checks.master.as_str());
		}
		report_field(STRUCTURE_STATUS_KEY, report.coverage.structure.as_str());
		report_field(PAYLOAD_STATUS_KEY, report.coverage.payload.as_str());
		report_field(FILESYSTEM_STATUS_KEY, report.coverage.filesystem.as_str());
		report_field(VERIFICATION_KEY, outcome(report.outcome));
	}
	if report.passed() {
		return Ok(());
	}
	Err(Box::new(VerificationFailure(report)))
}

pub(crate) fn unwrap(path: &Path, destination: &Path) -> CliResult {
	let mut image = Image::open(open_source(path)?, Limits::default())?;
	publish_new(
		destination,
		|stage| image.write_raw_to(stage),
		|_, _| Ok(()),
	)?;
	Ok(())
}

fn print_json(report: &impl Serialize) -> CliResult {
	let mut stdout = io::stdout().lock();
	serde_json::to_writer(&mut stdout, report)?;
	writeln!(stdout)?;
	Ok(())
}

fn checksum(value: ChecksumDeclaration) -> String {
	match value {
		ChecksumDeclaration::Absent => CheckStatus::NotPresent.as_str().into(),
		ChecksumDeclaration::Crc32 { value } => format!("{CRC32}:{value:08x}"),
		ChecksumDeclaration::Unsupported { kind, size_bits } => {
			format!("{}({kind}, {size_bits})", CheckStatus::Unsupported.as_str())
		}
		_ => UNKNOWN.into(),
	}
}

fn outcome(value: VerificationOutcome) -> &'static str {
	match value {
		VerificationOutcome::Passed => CheckStatus::Passed.as_str(),
		VerificationOutcome::Partial => PARTIAL,
		VerificationOutcome::Failed => CheckStatus::Failed.as_str(),
		VerificationOutcome::Unsupported => CheckStatus::Unsupported.as_str(),
		VerificationOutcome::Unavailable => CheckStatus::Unavailable.as_str(),
		_ => UNKNOWN,
	}
}

#[derive(Debug)]
struct VerificationFailure(VerificationReport);

impl fmt::Display for VerificationFailure {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"image verification outcome: {}",
			outcome(self.0.outcome)
		)
	}
}

impl Error for VerificationFailure {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		self.0.cause().map(|error| error as &dyn Error)
	}
}
