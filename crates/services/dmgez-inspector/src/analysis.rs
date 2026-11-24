use dmgez_udif::reader::{Image, ReadAt, RunDescriptor, RunEncoding};
use serde::Serialize;

use crate::{Coverage, ReportOperation, SCHEMA_VERSION};

/// Exact space accounting for a group of normalized runs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RunSpace {
	pub count: usize,
	#[serde(serialize_with = "crate::decimal")]
	pub logical_bytes: u64,
	#[serde(serialize_with = "crate::decimal")]
	pub stored_bytes: u64,
}

impl RunSpace {
	fn record(&mut self, run: &RunDescriptor) {
		self.count += 1;
		self.logical_bytes += run.logical.end - run.logical.start;
		self.stored_bytes += run.physical.end - run.physical.start;
	}
}

/// Space grouped by on-disk encoding, independent of compiled decoder availability.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RunSpaceByEncoding {
	pub raw: RunSpace,
	pub zero: RunSpace,
	pub ignore: RunSpace,
	pub zlib: RunSpace,
	pub lzfse: RunSpace,
	pub lzma2: RunSpace,
	pub other: RunSpace,
}

impl RunSpaceByEncoding {
	fn record(&mut self, run: &RunDescriptor) {
		let group = match run.encoding {
			RunEncoding::Raw => &mut self.raw,
			RunEncoding::Zero => &mut self.zero,
			RunEncoding::Ignore => &mut self.ignore,
			RunEncoding::Zlib => &mut self.zlib,
			RunEncoding::Lzfse => &mut self.lzfse,
			RunEncoding::Lzma2 => &mut self.lzma2,
			_ => &mut self.other,
		};
		group.record(run);
	}
}

/// On-disk space computed from admitted container metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AnalysisReport {
	pub schema_version: u32,
	pub operation: ReportOperation,
	/// Complete physical image length. Divide by `runs.logical_bytes` for its storage ratio.
	#[serde(serialize_with = "crate::decimal")]
	pub container_bytes: u64,
	/// Bytes outside the data fork, including metadata and any intervening gaps.
	#[serde(serialize_with = "crate::decimal")]
	pub outside_data_fork_bytes: u64,
	/// Bytes inside the data fork that no normalized run references.
	#[serde(serialize_with = "crate::decimal")]
	pub unmapped_data_fork_bytes: u64,
	/// Sum across all runs. The mapped storage ratio is `stored_bytes / logical_bytes`.
	pub runs: RunSpace,
	pub by_encoding: RunSpaceByEncoding,
	pub coverage: Coverage,
}

/// Summarizes observed disk-image space in one pass over the checked run index.
///
/// This performs no I/O or payload decoding. The report makes no claim about
/// filesystem allocation, content sizes, verification, or historical build timings.
#[must_use]
pub fn analyze(image: &Image<impl ReadAt>) -> AnalysisReport {
	let descriptor = image.descriptor();
	let data_fork_bytes = descriptor.data_fork.end - descriptor.data_fork.start;
	let mut runs = RunSpace::default();
	let mut by_encoding = RunSpaceByEncoding::default();
	// Admission partitions the logical disk and forbids physical run overlap.
	for run in image.runs() {
		runs.record(&run);
		by_encoding.record(&run);
	}
	AnalysisReport {
		schema_version: SCHEMA_VERSION,
		operation: ReportOperation::Analyze,
		container_bytes: descriptor.physical_bytes,
		outside_data_fork_bytes: descriptor.physical_bytes - data_fork_bytes,
		unmapped_data_fork_bytes: data_fork_bytes - runs.stored_bytes,
		runs,
		by_encoding,
		coverage: Coverage::default(),
	}
}
