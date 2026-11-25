use std::cell::Cell;

use dmgez_inspector::{ContainerSummary, VerificationMode, inspect, verify};
use dmgez_udif::{
	Error, TRAILER_BYTES,
	reader::{FileSource, Image, Limits, ReadAt},
};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

use crate::local::{FileAdmission, LocalRoot, direct_name};

const FILE_EXTENSION: &str = "dmg";
const MAX_IMAGE_BYTES: u64 = 1 << 40;
const MAX_XML_BYTES: usize = 1 << 20;
const MAX_TABLES: usize = 512;
const MAX_RECORDS: usize = 8192;
const MAX_DECODE_BYTES: usize = 32 << 20;
pub(super) const MAX_METADATA_BYTES: usize = MAX_XML_BYTES + TRAILER_BYTES;
const MAX_METADATA_READS: usize = 2;
const READ_BUDGET: &str = "MCP image metadata read budget";
const IMAGE_LIMITS: Limits = Limits {
	max_physical_bytes: MAX_IMAGE_BYTES,
	max_logical_bytes: MAX_IMAGE_BYTES,
	max_xml_bytes: MAX_XML_BYTES,
	max_tables: MAX_TABLES,
	max_records: MAX_RECORDS,
	max_compressed_bytes: MAX_DECODE_BYTES,
	max_decoded_bytes: MAX_DECODE_BYTES,
};

#[derive(Clone, Copy)]
pub(super) enum Operation {
	Inspect,
	Verify,
}

#[derive(Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Outcome {
	Inspected {
		summary: Summary,
	},
	Verified {
		summary: Summary,
		coverage: QuickCoverage,
	},
	Rejected {
		issue: Issue,
	},
	Busy,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct Summary {
	physical_bytes: u64,
	logical_bytes: u64,
	sector_bytes: u64,
	block_map_count: usize,
	run_count: usize,
	unsupported_run_count: usize,
	decoder_unavailable_count: usize,
}

impl From<&ContainerSummary> for Summary {
	fn from(container: &ContainerSummary) -> Self {
		Self {
			physical_bytes: container.physical_bytes,
			logical_bytes: container.logical_bytes,
			sector_bytes: container.sector_bytes,
			block_map_count: container.block_map_count,
			run_count: container.runs.total,
			unsupported_run_count: container.runs.unsupported,
			decoder_unavailable_count: container.runs.decode_unavailable,
		}
	}
}

#[derive(Serialize, JsonSchema)]
pub(super) struct QuickCoverage {
	structure: StructurePassed,
	payload: NotChecked,
	filesystem: NotChecked,
	security: NotChecked,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum StructurePassed {
	Passed,
}

#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum NotChecked {
	NotChecked,
}

#[derive(Serialize, JsonSchema)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(super) enum Issue {
	InvalidFileName,
	ImageUnavailable,
	ImageChanged,
	InvalidFormat,
	UnsupportedProfile,
	CorruptImage,
	ResourceLimit,
}

impl From<FileAdmission> for Issue {
	fn from(error: FileAdmission) -> Self {
		match error {
			FileAdmission::Unavailable => Self::ImageUnavailable,
			FileAdmission::Changed => Self::ImageChanged,
		}
	}
}

impl From<&Error> for Issue {
	fn from(error: &Error) -> Self {
		match error {
			Error::SourceChanged => Self::ImageChanged,
			Error::Io(_) => Self::ImageUnavailable,
			Error::InvalidFormat(_) => Self::InvalidFormat,
			Error::Unsupported(_) | Error::UnsupportedRun { .. } | Error::FeatureNotCompiled(_) => {
				Self::UnsupportedProfile
			}
			Error::ResourceLimit(_) => Self::ResourceLimit,
			_ => Self::CorruptImage,
		}
	}
}

impl LocalRoot {
	pub(super) fn image_operation(&self, file_name: &str, operation: Operation) -> Outcome {
		self.read_image(file_name, operation)
			.unwrap_or_else(|issue| Outcome::Rejected { issue })
	}

	fn read_image(&self, file_name: &str, operation: Operation) -> Result<Outcome, Issue> {
		let path = direct_name(file_name, FILE_EXTENSION).ok_or(Issue::InvalidFileName)?;
		let (file, binding) = self.open_file(path)?;
		let source = MetadataSource {
			file: FileSource::new(file).map_err(|error| Issue::from(&error))?,
			remaining_bytes: Cell::new(MAX_METADATA_BYTES),
			remaining_reads: Cell::new(MAX_METADATA_READS),
		};
		let result = match operation {
			Operation::Inspect => inspect_image(&source),
			Operation::Verify => verify_image(&source),
		};
		source
			.file
			.check_unchanged()
			.map_err(|error| Issue::from(&error))?;
		binding.verify()?;
		result
	}
}

fn inspect_image(source: &MetadataSource) -> Result<Outcome, Issue> {
	let image = Image::open(source, IMAGE_LIMITS).map_err(|error| Issue::from(&error))?;
	let report = inspect(&image, None);
	Ok(Outcome::Inspected {
		summary: (&report.container).into(),
	})
}

fn verify_image(source: &MetadataSource) -> Result<Outcome, Issue> {
	let report = verify(source, IMAGE_LIMITS, VerificationMode::Quick);
	let container = report
		.container
		.as_ref()
		.filter(|_| report.passed())
		.ok_or_else(|| report.cause().map_or(Issue::ImageUnavailable, Issue::from))?;
	Ok(Outcome::Verified {
		summary: container.into(),
		coverage: QuickCoverage {
			structure: StructurePassed::Passed,
			payload: NotChecked::NotChecked,
			filesystem: NotChecked::NotChecked,
			security: NotChecked::NotChecked,
		},
	})
}

struct MetadataSource {
	file: FileSource,
	remaining_bytes: Cell<usize>,
	remaining_reads: Cell<usize>,
}

impl ReadAt for &MetadataSource {
	fn len(&self) -> u64 {
		self.file.len()
	}

	fn check_unchanged(&self) -> Result<(), Error> {
		self.file.check_unchanged()
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		if !output.is_empty() {
			let bytes = self
				.remaining_bytes
				.get()
				.checked_sub(output.len())
				.ok_or(Error::ResourceLimit(READ_BUDGET))?;
			let reads = self
				.remaining_reads
				.get()
				.checked_sub(1)
				.ok_or(Error::ResourceLimit(READ_BUDGET))?;
			self.remaining_bytes.set(bytes);
			self.remaining_reads.set(reads);
		}
		self.file.read_exact_at(offset, output)
	}
}
