use std::{fmt, ops::Range};

use dmgez_udif::{
	SECTOR_BYTES,
	reader::{Checksum, Descriptor, Image, ReadAt, RunDescriptor, RunEncoding as UdifEncoding},
};
use serde::{Serialize, Serializer};

use crate::{CAPABILITIES, Capabilities, Coverage, DiskReport, ReportOperation, SCHEMA_VERSION};

const SINGLE_SEGMENT_COUNT: u32 = 1;

/// Maximum number of runs in one optional detail page.
pub const MAX_RUN_PAGE_SIZE: usize = 1024;

/// Half-open byte interval. Its parent field identifies the address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ByteRange {
	#[serde(serialize_with = "crate::decimal")]
	pub offset_bytes: u64,
	#[serde(serialize_with = "crate::decimal")]
	pub length_bytes: u64,
}

impl ByteRange {
	pub(crate) fn admitted(range: &Range<u64>) -> Self {
		Self {
			offset_bytes: range.start,
			length_bytes: range.end - range.start,
		}
	}
}

/// Raw on-disk segment identifier. This field is not a content digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentId(pub [u8; 16]);

impl fmt::Display for SegmentId {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		for byte in self.0 {
			write!(formatter, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl Serialize for SegmentId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(self)
	}
}

/// Admitted container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ContainerFormat {
	UdifV4,
}

/// Checksum descriptor observed in metadata, before comparing any bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "algorithm", rename_all = "snake_case")]
pub enum ChecksumDeclaration {
	Absent,
	Crc32 { value: u32 },
	Unsupported { kind: u32, size_bits: u32 },
}

impl From<Checksum> for ChecksumDeclaration {
	fn from(checksum: Checksum) -> Self {
		match checksum {
			Checksum::Absent => Self::Absent,
			Checksum::Crc32(value) => Self::Crc32 { value },
			Checksum::Unsupported { kind, size_bits } => Self::Unsupported { kind, size_bits },
		}
	}
}

/// Bounded summary without resource plist contents or partition assumptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ContainerSummary {
	pub format: ContainerFormat,
	pub segment_count: u32,
	pub segment_id: SegmentId,
	pub variant: u32,
	pub sector_bytes: u64,
	#[serde(serialize_with = "crate::decimal")]
	pub physical_bytes: u64,
	#[serde(serialize_with = "crate::decimal")]
	pub logical_bytes: u64,
	pub data_fork: ByteRange,
	pub resource_xml: ByteRange,
	pub data_fork_checksum: ChecksumDeclaration,
	pub master_checksum: ChecksumDeclaration,
	/// Count of UDIF block maps. Partition discovery is a separate operation.
	pub block_map_count: usize,
	pub runs: RunSummary,
}

impl ContainerSummary {
	pub(crate) fn read<S: ReadAt>(image: &Image<S>) -> Self {
		let descriptor = image.descriptor();
		let runs = image.runs().fold(RunSummary::default(), |mut counts, run| {
			counts.record(run.encoding);
			counts
		});
		Self::from_descriptor(descriptor, runs)
	}

	fn from_descriptor(descriptor: &Descriptor, runs: RunSummary) -> Self {
		Self {
			format: ContainerFormat::UdifV4,
			segment_count: SINGLE_SEGMENT_COUNT,
			segment_id: SegmentId(descriptor.segment_id),
			variant: descriptor.variant,
			sector_bytes: SECTOR_BYTES,
			physical_bytes: descriptor.physical_bytes,
			logical_bytes: descriptor.logical_bytes,
			data_fork: ByteRange::admitted(&descriptor.data_fork),
			resource_xml: ByteRange::admitted(&descriptor.resource_xml),
			data_fork_checksum: descriptor.physical_crc.into(),
			master_checksum: descriptor.master_crc.into(),
			block_map_count: descriptor.tables.len(),
			runs,
		}
	}
}

/// Counts of run types observed without decoding their payload.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RunSummary {
	pub total: usize,
	pub raw: usize,
	pub zero: usize,
	pub ignore: usize,
	pub zlib: usize,
	pub lzfse: usize,
	pub unsupported: usize,
	/// Runs whose codec is unknown or whose decoder is disabled in this build.
	pub decode_unavailable: usize,
}

impl RunSummary {
	fn record(&mut self, encoding: UdifEncoding) {
		self.total += 1;
		self.decode_unavailable += usize::from(!encoding.can_decode());
		match encoding {
			UdifEncoding::Raw => self.raw += 1,
			UdifEncoding::Zero => self.zero += 1,
			UdifEncoding::Ignore => self.ignore += 1,
			UdifEncoding::Zlib => self.zlib += 1,
			UdifEncoding::Lzfse => self.lzfse += 1,
			_ => self.unsupported += 1,
		}
	}
}

/// Observed run encoding. Decode availability is determined by verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEncoding {
	Raw,
	Zero,
	Ignore,
	Zlib,
	Lzfse,
	Unsupported { code: u32 },
	Unreported,
}

impl From<UdifEncoding> for RunEncoding {
	fn from(encoding: UdifEncoding) -> Self {
		match encoding {
			UdifEncoding::Raw => Self::Raw,
			UdifEncoding::Zero => Self::Zero,
			UdifEncoding::Ignore => Self::Ignore,
			UdifEncoding::Zlib => Self::Zlib,
			UdifEncoding::Lzfse => Self::Lzfse,
			UdifEncoding::Unsupported(code) => Self::Unsupported { code },
			_ => Self::Unreported,
		}
	}
}

/// Logical run and its entire stored byte range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Run {
	pub logical: ByteRange,
	pub physical: ByteRange,
	pub encoding: RunEncoding,
	pub decoder_available: bool,
}

impl From<RunDescriptor> for Run {
	fn from(run: RunDescriptor) -> Self {
		Self {
			logical: ByteRange::admitted(&run.logical),
			physical: ByteRange::admitted(&run.physical),
			encoding: run.encoding.into(),
			decoder_available: run.encoding.can_decode(),
		}
	}
}

/// Request for a bounded page of runs in logical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunWindow {
	offset: usize,
	limit: usize,
}

impl RunWindow {
	/// A zero limit or one above [`MAX_RUN_PAGE_SIZE`] is invalid.
	#[must_use]
	pub const fn new(offset: usize, limit: usize) -> Option<Self> {
		if limit == 0 || limit > MAX_RUN_PAGE_SIZE {
			return None;
		}
		Some(Self { offset, limit })
	}
}

/// Bounded detail page for the requested logical run window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RunPage {
	#[serde(serialize_with = "crate::decimal")]
	pub offset: u64,
	pub next_offset: Option<usize>,
	pub runs: Vec<Run>,
}

/// Report obtained from already admitted UDIF metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct InspectionReport {
	pub schema_version: u32,
	pub operation: ReportOperation,
	pub container: ContainerSummary,
	pub capabilities: Capabilities,
	pub coverage: Coverage,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub run_page: Option<RunPage>,
	/// Result of optional bounded partition discovery on the logical disk.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub disk: Option<DiskReport>,
}

/// Summarizes an opened image without reading or verifying payload bytes.
#[must_use]
pub fn inspect<S: ReadAt>(image: &Image<S>, window: Option<RunWindow>) -> InspectionReport {
	let container = ContainerSummary::read(image);
	let run_page = window.map(|window| {
		let runs: Vec<_> = image
			.runs()
			.skip(window.offset)
			.take(window.limit)
			.map(Run::from)
			.collect();
		let next = window.offset + runs.len();
		RunPage {
			offset: window.offset as u64,
			next_offset: (next < container.runs.total).then_some(next),
			runs,
		}
	});
	InspectionReport {
		schema_version: SCHEMA_VERSION,
		operation: ReportOperation::Inspect,
		container,
		capabilities: CAPABILITIES,
		coverage: Coverage::default(),
		run_page,
		disk: None,
	}
}
