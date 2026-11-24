use std::{fs::File, path::Path};

use dmgez_udif::reader::{CheckStatus, Checksum, FileSource, Image, Limits, RunEncoding};

use crate::{CliResult, output, report_field};

const FORMAT_KEY: &str = "format";
const PHYSICAL_BYTES_KEY: &str = "physical_bytes";
const LOGICAL_BYTES_KEY: &str = "logical_bytes";
const VARIANT_KEY: &str = "variant";
const BLOCK_MAP_COUNT_KEY: &str = "block_maps";
const DATA_FORK_CHECKSUM_KEY: &str = "data_fork_checksum";
const MASTER_CHECKSUM_KEY: &str = "master_checksum";
const UNSUPPORTED_RUNS_KEY: &str = "unsupported_runs";
const VERIFICATION_KEY: &str = "verification";
const DATA_FORK_STATUS_KEY: &str = "data_fork";
const BLOCK_MAP_STATUS_KEY: &str = "block_map";
const MASTER_STATUS_KEY: &str = "master";
const FILESYSTEM_STATUS_KEY: &str = "filesystem";
const UDIF_V4: &str = "UDIF v4";
const CRC32: &str = "crc32";
const UNSUPPORTED: &str = "unsupported";
const PASSED: &str = "passed";
const NOT_CHECKED: &str = "not_checked";
const NOT_PRESENT: &str = "not_present";

fn open(path: &Path) -> Result<Image<FileSource>, dmgez_udif::Error> {
	let source = FileSource::new(File::open(path)?)?;
	Image::open(source, Limits::default())
}

pub(crate) fn show(path: &Path) -> CliResult {
	let image = open(path)?;
	let descriptor = image.descriptor();
	report_field(FORMAT_KEY, UDIF_V4);
	report_field(PHYSICAL_BYTES_KEY, descriptor.physical_bytes);
	report_field(LOGICAL_BYTES_KEY, descriptor.logical_bytes);
	report_field(VARIANT_KEY, descriptor.variant);
	report_field(BLOCK_MAP_COUNT_KEY, descriptor.tables.len());
	report_field(DATA_FORK_CHECKSUM_KEY, checksum(descriptor.physical_crc));
	report_field(MASTER_CHECKSUM_KEY, checksum(descriptor.master_crc));
	report_field(
		UNSUPPORTED_RUNS_KEY,
		image
			.runs()
			.filter(|run| matches!(run.encoding, RunEncoding::Unsupported(_)))
			.count(),
	);
	report_field(VERIFICATION_KEY, NOT_CHECKED);
	Ok(())
}

pub(crate) fn verify(path: &Path) -> CliResult {
	let verified = open(path)?.verify()?;
	report_field(DATA_FORK_STATUS_KEY, status(verified.data_fork));
	for (index, check) in verified.tables.into_iter().enumerate() {
		report_field(
			format_args!("{BLOCK_MAP_STATUS_KEY}[{index}]"),
			status(check),
		);
	}
	report_field(MASTER_STATUS_KEY, status(verified.master));
	report_field(FILESYSTEM_STATUS_KEY, NOT_CHECKED);
	Ok(())
}

pub(crate) fn unwrap(path: &Path, destination: &Path) -> CliResult {
	let mut image = open(path)?;
	output::publish(destination, |stage| Ok(image.write_raw_to(stage)?))
}

fn checksum(value: Checksum) -> String {
	match value {
		Checksum::Absent => NOT_PRESENT.into(),
		Checksum::Crc32(crc) => format!("{CRC32}:{crc:08x}"),
		Checksum::Unsupported { kind, size_bits } => format!("{UNSUPPORTED}({kind}, {size_bits})"),
	}
}

fn status(check: CheckStatus) -> &'static str {
	match check {
		CheckStatus::Passed => PASSED,
		CheckStatus::NotPresent => NOT_PRESENT,
	}
}
