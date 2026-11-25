use std::{
	error::Error,
	fmt,
	fs::File,
	io::{self, Write},
	num::NonZeroU32,
	path::Path,
};

use dmgez_core::output::publish_new;
use dmgez_inspector::{
	CheckStatus, ChecksumDeclaration, DiagnosticDetail, DiskLimits, DiskReport, MAX_RUN_PAGE_SIZE,
	RunWindow, VerificationMode, VerificationOutcome, VerificationReport,
};
use dmgez_udif::reader::{FileSource, Image, Limits, ReadAt};
use dmgez_vfs::{Disk, Layout, PartitionId, VolumeId};
use serde::Serialize;

use crate::{CliResult, inspect_command::Arguments, verify_command, volume_commands::Filesystem};

#[cfg(feature = "apfs-read")]
mod apfs;
#[cfg(feature = "hfsplus-verify")]
mod hfsplus;

const FORMAT_KEY: &str = "format";
const PHYSICAL_BYTES_KEY: &str = "physical_bytes";
const LOGICAL_BYTES_KEY: &str = "logical_bytes";
const VARIANT_KEY: &str = "variant";
const BLOCK_MAP_COUNT_KEY: &str = "block_maps";
const EMBEDDED_SIGNATURE_KEY: &str = "embedded_signature";
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
const SECURITY_STATUS_KEY: &str = "security";
const FILESYSTEM_SCOPE_KEY: &str = "filesystem_scope";
const FILESYSTEM_VOLUME_UUID_KEY: &str = "filesystem_volume_uuid";
const FILESYSTEM_VOLUME_OBJECT_ID_KEY: &str = "filesystem_volume_object_id";
const CHECKPOINT_XID_KEY: &str = "checkpoint_xid";
const CHECKPOINT_REJECTED_DESCRIPTORS_KEY: &str = "checkpoint_rejected_descriptors";
const ADVERTISED_CHECKPOINT_XID_KEY: &str = "advertised_checkpoint_xid";
const LOGICAL_PAYLOAD_BYTES_READ_KEY: &str = "logical_payload_bytes_read";
const STORED_PAYLOAD_BYTES_READ_KEY: &str = "stored_payload_bytes_read";
const UDIF_V4: &str = "UDIF v4";
const CRC32: &str = "crc32";
const UNKNOWN: &str = "unknown";
const PARTIAL: &str = "partial";
const OUTSIDE_DATA_FORK_KEY: &str = "outside_data_fork_bytes";
const UNMAPPED_DATA_FORK_KEY: &str = "unmapped_data_fork_bytes";
const MAPPED_STORAGE_RATIO_KEY: &str = "mapped_storage_ratio";
const CONTAINER_STORAGE_RATIO_KEY: &str = "container_storage_ratio";
const RAW_ENCODING: &str = "raw";
const ZERO_ENCODING: &str = "zero";
const IGNORE_ENCODING: &str = "ignore";
const ZLIB_ENCODING: &str = "zlib";
const LZFSE_ENCODING: &str = "lzfse";
const LZMA2_ENCODING: &str = "lzma2";
const OTHER_ENCODING: &str = "other";
const PARTITION_COPY_BUFFER_BYTES: usize = 64 * 1024;

fn report_field(key: impl std::fmt::Display, value: impl std::fmt::Display) {
	println!("{key} = {value}");
}

pub(super) fn open_source(path: &Path) -> Result<FileSource, dmgez_udif::Error> {
	FileSource::new(File::open(path)?)
}

pub(super) fn show<S: ReadAt>(
	arguments: &Arguments,
	open: impl FnOnce() -> Result<S, Box<dyn Error>>,
) -> CliResult {
	let window = match arguments.run_limit {
		Some(limit) => Some(
			RunWindow::new(arguments.run_offset.unwrap_or(0), limit).ok_or_else(|| {
				io::Error::new(
					io::ErrorKind::InvalidInput,
					format!("run limit must be between 1 and {MAX_RUN_PAGE_SIZE}"),
				)
			})?,
		),
		None => None,
	};
	let mut image = Image::open_release(open()?, Limits::default())?;
	let report = if arguments.partitions {
		dmgez_inspector::inspect_disk(&mut image, window, DiskLimits::default())
	} else {
		dmgez_inspector::inspect(&image, window)
	};
	image.check_source_unchanged()?;
	if arguments.json {
		return print_json(&report);
	}
	let container = report.container;
	report_field(FORMAT_KEY, UDIF_V4);
	report_field(PHYSICAL_BYTES_KEY, container.physical_bytes);
	report_field(LOGICAL_BYTES_KEY, container.logical_bytes);
	report_field(VARIANT_KEY, container.variant);
	report_field(BLOCK_MAP_COUNT_KEY, container.block_map_count);
	if let Some(signature) = container.embedded_signature {
		report_field(
			EMBEDDED_SIGNATURE_KEY,
			format_args!(
				"stored byte {} length {}",
				signature.offset_bytes, signature.length_bytes,
			),
		);
	}
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

pub(crate) fn analyze(path: &Path, json: bool) -> CliResult {
	let image = Image::open_release(open_source(path)?, Limits::default())?;
	let report = dmgez_inspector::analyze(&image);
	image.check_source_unchanged()?;
	if json {
		return print_json(&report);
	}
	write_analysis(&mut io::stdout().lock(), &report)?;
	Ok(())
}

pub(super) fn write_analysis(
	output: &mut impl Write,
	report: &dmgez_inspector::AnalysisReport,
) -> io::Result<()> {
	writeln!(output, "{PHYSICAL_BYTES_KEY} = {}", report.container_bytes)?;
	writeln!(
		output,
		"{LOGICAL_BYTES_KEY} = {}",
		report.runs.logical_bytes
	)?;
	writeln!(
		output,
		"{OUTSIDE_DATA_FORK_KEY} = {}",
		report.outside_data_fork_bytes
	)?;
	writeln!(
		output,
		"{UNMAPPED_DATA_FORK_KEY} = {}",
		report.unmapped_data_fork_bytes
	)?;
	writeln!(
		output,
		"{MAPPED_STORAGE_RATIO_KEY} = {}/{}",
		report.runs.stored_bytes, report.runs.logical_bytes,
	)?;
	writeln!(
		output,
		"{CONTAINER_STORAGE_RATIO_KEY} = {}/{}",
		report.container_bytes, report.runs.logical_bytes,
	)?;
	let groups = report.by_encoding;
	for (encoding, group) in [
		(RAW_ENCODING, groups.raw),
		(ZERO_ENCODING, groups.zero),
		(IGNORE_ENCODING, groups.ignore),
		(ZLIB_ENCODING, groups.zlib),
		(LZFSE_ENCODING, groups.lzfse),
		(LZMA2_ENCODING, groups.lzma2),
		(OTHER_ENCODING, groups.other),
	] {
		if group.count > 0 {
			writeln!(
				output,
				"{encoding} = {} runs, {} logical bytes, {} stored bytes",
				group.count, group.logical_bytes, group.stored_bytes,
			)?;
		}
	}
	writeln!(
		output,
		"{VERIFICATION_KEY} = {}",
		report.coverage.payload.as_str()
	)
}

pub(super) fn verify(arguments: &verify_command::Arguments, mode: VerificationMode) -> CliResult {
	#[cfg(not(feature = "apfs-read"))]
	if arguments.raw {
		let feature = crate::volume_commands::APFS_READ_FEATURE;
		return Err(format!("feature not compiled: {feature}").into());
	}

	let (report, filesystem_cause) = match arguments.selection.filesystem() {
		Filesystem::Hfsplus => verify_hfsplus(arguments, mode)?,
		#[cfg(feature = "apfs-read")]
		Filesystem::Apfs => apfs::verify(arguments)?,
		#[cfg(not(feature = "apfs-read"))]
		Filesystem::Apfs => (
			dmgez_inspector::verify(open_source(&arguments.image)?, Limits::default(), mode),
			None,
		),
	};
	if arguments.json {
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
		report_field(SECURITY_STATUS_KEY, report.coverage.security.as_str());
		show_filesystem_coverage(&report);
		report_field(VERIFICATION_KEY, outcome(report.outcome));
	}
	if report.passed() {
		return Ok(());
	}
	Err(Box::new(VerificationFailure {
		report,
		filesystem_cause,
	}))
}

fn verify_hfsplus(
	arguments: &verify_command::Arguments,
	mode: VerificationMode,
) -> CliResult<(VerificationReport, Option<Box<dyn Error>>)> {
	let source = open_source(&arguments.image)?;
	#[cfg(feature = "hfsplus-verify")]
	if mode == VerificationMode::Full {
		return Ok(hfsplus::verify(
			source,
			arguments.partition,
			arguments.max_payload_bytes(),
		));
	}
	let mut report = dmgez_inspector::verify(source, Limits::default(), mode);
	report.capabilities.filesystem = cfg!(feature = "hfsplus-verify");
	Ok((report, None))
}

fn show_filesystem_coverage(report: &VerificationReport) {
	let Some(evidence) = report
		.filesystem_check
		.as_ref()
		.and_then(|check| check.evidence)
	else {
		return;
	};
	report_field(FILESYSTEM_SCOPE_KEY, evidence.scope.as_str());
	if let Some(uuid) = evidence.volume_uuid {
		report_field(FILESYSTEM_VOLUME_UUID_KEY, uuid);
	}
	if let Some(id) = evidence.volume_object_id {
		report_field(FILESYSTEM_VOLUME_OBJECT_ID_KEY, id.0);
	}
	if let Some(xid) = evidence.checkpoint_xid {
		report_field(CHECKPOINT_XID_KEY, xid.0);
	}
	if let Some(bytes) = evidence.logical_payload_bytes_read {
		report_field(LOGICAL_PAYLOAD_BYTES_READ_KEY, bytes);
	}
	if let Some(bytes) = evidence.stored_payload_bytes_read {
		report_field(STORED_PAYLOAD_BYTES_READ_KEY, bytes);
	}
	if let Some(checkpoints) = evidence.checkpoint_diagnostics {
		report_field(
			CHECKPOINT_REJECTED_DESCRIPTORS_KEY,
			checkpoints.rejected_descriptors,
		);
		if let Some(xid) = checkpoints.advertised_xid {
			report_field(ADVERTISED_CHECKPOINT_XID_KEY, xid.0);
		}
	}
}

pub(crate) fn unwrap(path: &Path, destination: &Path, partition: Option<NonZeroU32>) -> CliResult {
	let mut image = Image::open_release(open_source(path)?, Limits::default())?;
	if let Some(slot) = partition {
		let mut disk = Disk::discover(&mut image, DiskLimits::default())?;
		let Layout::Partitioned { scheme, .. } = *disk.layout() else {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"partition export requires a recognized partition map",
			)
			.into());
		};
		let mut partition = disk.window(VolumeId::Partition(PartitionId { scheme, slot }))?;
		let mut scratch = vec![0; PARTITION_COPY_BUFFER_BYTES];
		publish_new(
			destination,
			|stage| partition.write_raw_to(stage, &mut scratch, || false),
			|_, _| Ok(()),
		)?;
		return Ok(());
	}
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
struct VerificationFailure {
	report: VerificationReport,
	filesystem_cause: Option<Box<dyn Error>>,
}

impl fmt::Display for VerificationFailure {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"image verification outcome: {}",
			outcome(self.report.outcome)
		)?;
		if matches!(
			self.report
				.diagnostic
				.as_ref()
				.and_then(|value| value.detail.as_ref()),
			Some(DiagnosticDetail::FilesystemVolumeSelection { .. })
		) {
			let option = crate::volume_commands::VOLUME_OPTION;
			let value_name = crate::volume_commands::VOLUME_VALUE_NAME;
			write!(
				formatter,
				". Select an APFS volume with --{option} {value_name}"
			)?;
		}
		Ok(())
	}
}

impl Error for VerificationFailure {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		self.report
			.cause()
			.map(|error| error as &dyn Error)
			.or(self.filesystem_cause.as_deref())
	}
}
