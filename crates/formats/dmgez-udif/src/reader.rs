//! Bounded metadata inspection, random reads and checksum verification.

#[cfg(feature = "write")]
mod rebuild;
mod release;
mod source;

use std::{io::Write, ops::Range};

use crc32fast::Hasher;
pub use plist::Value as ResourceValue;

use crate::{
	Error, SECTOR_BYTES,
	blkx::{Kind, Table},
	checksum,
	index::{self, Run},
	resource, trailer,
};

pub use crate::checksum::Checksum;
use source::admitted_range;
pub use source::{FileSource, ReadAt};

const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_PHYSICAL_BYTES: u64 = 1 << 40;
const DEFAULT_MAX_LOGICAL_BYTES: u64 = 1 << 40;
const DEFAULT_MAX_XML_BYTES: usize = 16 << 20;
const DEFAULT_MAX_TABLES: usize = 4096;
const DEFAULT_MAX_RECORDS: usize = 131_072;
const DEFAULT_MAX_COMPRESSED_BYTES: usize = 32 << 20;
const DEFAULT_MAX_DECODED_BYTES: usize = 32 << 20;

/// Resource limits used before allocating metadata or inflating a run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
	/// Maximum actual source length.
	pub max_physical_bytes: u64,
	/// Maximum declared logical disk size.
	pub max_logical_bytes: u64,
	/// Maximum XML resource bytes before decoding.
	pub max_xml_bytes: usize,
	/// Maximum number of block-map tables.
	pub max_tables: usize,
	/// Maximum total number of records across admitted tables, including control records.
	pub max_records: usize,
	/// Maximum stored size of a single compressed run.
	pub max_compressed_bytes: usize,
	/// Maximum expanded size of a single compressed run.
	pub max_decoded_bytes: usize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_physical_bytes: DEFAULT_MAX_PHYSICAL_BYTES,
			max_logical_bytes: DEFAULT_MAX_LOGICAL_BYTES,
			max_xml_bytes: DEFAULT_MAX_XML_BYTES,
			max_tables: DEFAULT_MAX_TABLES,
			max_records: DEFAULT_MAX_RECORDS,
			max_compressed_bytes: DEFAULT_MAX_COMPRESSED_BYTES,
			max_decoded_bytes: DEFAULT_MAX_DECODED_BYTES,
		}
	}
}

/// Metadata for a structurally admitted block-map table.
#[derive(Debug)]
#[non_exhaustive]
pub struct TableDescriptor {
	/// Resource ID, when stored as text.
	pub id: Option<String>,
	/// Logical byte interval in the image.
	pub logical: Range<u64>,
	/// Number of original records, including control records.
	pub records: usize,
	/// Number of sectors declared in the block-map buffer field.
	pub buffer_sectors: u32,
	/// Original block-map descriptor field.
	pub descriptor: u32,
	/// Declared checksum of stored logical table bytes.
	pub checksum: Checksum,
}

/// Metadata obtained without reading or inflating the data fork.
#[derive(Debug)]
#[non_exhaustive]
pub struct Descriptor {
	/// Actual image size in bytes.
	pub physical_bytes: u64,
	/// Declared logical disk size in bytes.
	pub logical_bytes: u64,
	/// Physical data-fork range.
	pub data_fork: Range<u64>,
	/// Physical XML resource range.
	pub resource_xml: Range<u64>,
	/// Raw 16-byte segment identity.
	pub segment_id: [u8; 16],
	/// Original image variant field.
	pub variant: u32,
	/// Declared data-fork checksum.
	pub physical_crc: Checksum,
	/// Declared checksum of the table CRC list.
	pub master_crc: Checksum,
	/// Tables in original resource order, which defines the master CRC domain.
	pub tables: Vec<TableDescriptor>,
	/// Parsed resource tree, including unrecognized keys and original block-map data.
	pub resources: ResourceValue,
}

/// Encoding of a run in the admitted logical disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunEncoding {
	/// Stored uncompressed bytes.
	Raw,
	/// Explicit logical zeros included in the table CRC.
	Zero,
	/// Unused logical space excluded from the table CRC. Reads return zeros.
	Ignore,
	/// Zlib compressed extent. Reading it requires `zlib` support.
	Zlib,
	/// LZFSE compressed extent. Reading it requires `lzfse` support.
	Lzfse,
	/// LZMA2 compressed extent in an XZ stream. Reading it requires `lzma` support.
	Lzma2,
	/// A recognized map contains an unimplemented run kind.
	Unsupported(u32),
}

impl RunEncoding {
	/// Whether the current build can decode this admitted run type.
	#[must_use]
	pub const fn can_decode(self) -> bool {
		match self {
			Self::Zlib => cfg!(feature = "zlib"),
			Self::Lzfse => cfg!(feature = "lzfse"),
			Self::Lzma2 => cfg!(feature = "lzma"),
			Self::Unsupported(_) => false,
			Self::Raw | Self::Zero | Self::Ignore => true,
		}
	}
}

/// One admitted run in logical order, before any payload decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunDescriptor {
	/// Disk-relative logical bytes used by the decoded view.
	pub logical: Range<u64>,
	/// Container-relative stored bytes. Zero and ignore entries have empty windows.
	pub physical: Range<u64>,
	/// Run encoding and decode availability.
	pub encoding: RunEncoding,
}

/// Outcome of checking one declared checksum domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
	/// A supported declaration matched the independently read bytes.
	Passed,
	/// The image omitted a checksum for this domain.
	NotPresent,
}

/// Full payload verification results. No filesystem checks are performed here.
#[derive(Debug)]
#[non_exhaustive]
pub struct Verification {
	/// Physical data-fork CRC status.
	pub data_fork: CheckStatus,
	/// Table CRC statuses in original resource order.
	pub tables: Vec<CheckStatus>,
	/// Master CRC status.
	pub master: CheckStatus,
}

/// An immutable disk image with a byte-bounded, single-run decode cache.
#[derive(Debug)]
pub struct Image<S> {
	source: S,
	body_len: u64,
	descriptor: Descriptor,
	runs: Vec<Run>,
	logical_order: Vec<usize>,
	table_runs: Vec<Range<usize>>,
	cache: Option<(usize, Vec<u8>)>,
}

impl<S: ReadAt> Image<S> {
	/// Open trailer and block-map metadata. Unknown run kinds are retained for
	/// inspection. Reading a range that depends on one returns an error.
	///
	/// # Errors
	/// Rejects malformed, out-of-budget or unsupported container metadata and I/O failures.
	pub fn open(source: S, limits: Limits) -> Result<Self, Error> {
		let (tail, body_len) = release::read_trailer(&source, limits)?;
		Self::open_from_trailer(source, limits, &tail, body_len)
	}

	fn open_from_trailer(
		source: S,
		limits: Limits,
		tail: &[u8; trailer::BYTES],
		body_len: u64,
	) -> Result<Self, Error> {
		let trailer = trailer::Trailer::decode(tail, body_len)?;
		if trailer.logical_bytes > limits.max_logical_bytes {
			return Err(Error::ResourceLimit("logical disk size"));
		}
		let xml_len = usize::try_from(trailer.xml.end - trailer.xml.start)
			.ok()
			.filter(|n| *n <= limits.max_xml_bytes)
			.ok_or(Error::ResourceLimit("resource XML bytes"))?;
		let mut xml = vec![0; xml_len];
		source.read_exact_at(trailer.xml.start, &mut xml)?;
		let resources = resource::decode(&xml)?;
		let tables = resource::tables(&resources)?;
		if tables.len() > limits.max_tables {
			return Err(Error::ResourceLimit("block-map tables"));
		}
		let mut remaining = limits.max_records;
		let mut descriptors = Vec::with_capacity(tables.len());
		let mut table_runs = Vec::with_capacity(tables.len());
		let mut runs = Vec::new();
		for entry in tables {
			let entry = entry
				.as_dictionary()
				.ok_or(Error::Corrupt("block-map resource is not a dictionary"))?;
			let data = entry
				.get(resource::BLOCK_MAP_DATA_KEY)
				.and_then(ResourceValue::as_data)
				.ok_or(Error::Corrupt("missing block-map data"))?;
			let table = Table::decode(data, remaining)?;
			remaining -= table.records.len();
			let first_run = runs.len();
			let normalized = index::normalize(&table, trailer.logical_bytes, &trailer.data)?;
			for run in &normalized {
				if run.kind.is_compressed() {
					if range_len(&run.physical) > limits.max_compressed_bytes as u64 {
						return Err(Error::ResourceLimit("compressed run size"));
					}
					if range_len(&run.logical) > limits.max_decoded_bytes as u64 {
						return Err(Error::ResourceLimit("decoded run size"));
					}
				}
			}
			runs.extend(normalized);
			table_runs.push(first_run..runs.len());
			let start = index::bytes(table.first_sector)?;
			descriptors.push(TableDescriptor {
				id: entry
					.get(resource::BLOCK_MAP_ID_KEY)
					.and_then(ResourceValue::as_string)
					.map(str::to_owned),
				logical: start..start + index::bytes(table.sector_count)?,
				records: table.records.len(),
				buffer_sectors: table.buffer_sectors,
				descriptor: table.descriptor,
				checksum: table.checksum,
			});
		}
		let logical_order = index::order_runs(&runs, trailer.logical_bytes)?;
		let descriptor = Descriptor {
			physical_bytes: source.len(),
			logical_bytes: trailer.logical_bytes,
			data_fork: trailer.data,
			resource_xml: trailer.xml,
			segment_id: trailer.segment_id,
			variant: trailer.variant,
			physical_crc: trailer.physical_crc,
			master_crc: trailer.master_crc,
			tables: descriptors,
			resources,
		};
		Ok(Self {
			source,
			body_len,
			descriptor,
			runs,
			logical_order,
			table_runs,
			cache: None,
		})
	}

	/// Metadata read when the image was opened.
	#[must_use]
	pub fn descriptor(&self) -> &Descriptor {
		&self.descriptor
	}

	/// Inspect the normalized map in logical order without reading any data fork payload.
	pub fn runs(&self) -> impl ExactSizeIterator<Item = RunDescriptor> + '_ {
		self.logical_order.iter().map(|&index| {
			let run = &self.runs[index];
			let encoding = match run.kind {
				Kind::Raw => RunEncoding::Raw,
				Kind::Zero => RunEncoding::Zero,
				Kind::Ignore => RunEncoding::Ignore,
				Kind::Zlib => RunEncoding::Zlib,
				Kind::Lzfse => RunEncoding::Lzfse,
				Kind::Lzma2 => RunEncoding::Lzma2,
				Kind::Unsupported(kind) => RunEncoding::Unsupported(kind),
				Kind::Comment | Kind::End => unreachable!("index contains only logical runs"),
			};
			RunDescriptor {
				logical: run.logical.clone(),
				physical: run.physical.clone(),
				encoding,
			}
		})
	}

	/// Read exactly a logical byte range, including reads across run boundaries.
	///
	/// # Errors
	/// Rejects out-of-range reads, missing codecs, bad compressed data and I/O failures.
	pub fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
		let range = admitted_range(offset, output.len() as u64, self.descriptor.logical_bytes)?;
		self.source.check_unchanged()?;
		let mut cursor = index::ReadCursor::new(&self.runs, &self.logical_order, range);
		while let Some((index, piece)) = cursor.next(&self.runs, &self.logical_order) {
			let target = (piece.start - offset) as usize..(piece.end - offset) as usize;
			self.read_run(index, piece.start, &mut output[target])?;
		}
		self.source.check_unchanged()
	}

	/// Write the complete logical disk, including zero reads over native ignore regions.
	///
	/// # Errors
	/// Returns a read or write error. On failure the caller-owned sink may contain a prefix.
	pub fn write_raw_to(&mut self, mut sink: impl Write) -> Result<(), Error> {
		let mut buffer = vec![0; STREAM_BUFFER_BYTES];
		let mut offset = 0;
		while offset < self.descriptor.logical_bytes {
			let len = self
				.descriptor
				.logical_bytes
				.saturating_sub(offset)
				.min(buffer.len() as u64) as usize;
			self.read_exact_at(offset, &mut buffer[..len])?;
			sink.write_all(&buffer[..len])?;
			offset += len as u64;
		}
		sink.flush()?;
		Ok(())
	}

	/// Verify all declared checksum domains, including decoding each stored run.
	///
	/// # Errors
	/// Returns an error for a mismatched or unsupported checksum, unsupported run, or failed I/O.
	pub fn verify(&mut self) -> Result<Verification, Error> {
		self.verify_with(|| Ok(()))
	}

	/// Verify the same checksum domains as [`Self::verify`], checking for cancellation
	/// before starting, between data chunks, and before reporting success.
	///
	/// # Errors
	/// Returns [`Error::Cancelled`] when cancellation is requested. Other errors
	/// are the same as in [`Self::verify`].
	pub fn verify_with_cancellation(
		&mut self,
		cancellation: &crate::Cancellation,
	) -> Result<Verification, Error> {
		self.verify_with(|| cancellation.check())
	}

	fn verify_with(
		&mut self,
		check_cancellation: impl Fn() -> Result<(), Error>,
	) -> Result<Verification, Error> {
		check_cancellation()?;
		// Require a fresh read of all verification inputs, including a cached compressed run.
		self.cache = None;
		let mut buffer = vec![0; STREAM_BUFFER_BYTES];
		let mut physical = Hasher::new();
		let mut offset = self.descriptor.data_fork.start;
		while offset < self.descriptor.data_fork.end {
			check_cancellation()?;
			let len = (self.descriptor.data_fork.end - offset).min(buffer.len() as u64) as usize;
			self.source.read_exact_at(offset, &mut buffer[..len])?;
			physical.update(&buffer[..len]);
			offset += len as u64;
		}
		let data_fork = compare(self.descriptor.physical_crc, physical.finalize(), || {
			"data fork".into()
		})?;
		let mut tables = Vec::with_capacity(self.table_runs.len());
		let mut declared = Vec::with_capacity(self.table_runs.len());
		for ordinal in 0..self.table_runs.len() {
			let mut crc = Hasher::new();
			for index in self.table_runs[ordinal].clone() {
				if self.runs[index].kind == Kind::Ignore {
					continue;
				}
				let range = self.runs[index].logical.clone();
				let mut offset = range.start;
				while offset < range.end {
					check_cancellation()?;
					let len = (range.end - offset).min(buffer.len() as u64) as usize;
					self.read_run(index, offset, &mut buffer[..len])?;
					crc.update(&buffer[..len]);
					offset += len as u64;
				}
			}
			let checksum = self.descriptor.tables[ordinal].checksum;
			tables.push(compare(checksum, crc.finalize(), || {
				format!("block map {ordinal}")
			})?);
			if let Checksum::Crc32(value) = checksum {
				declared.push(value);
			}
		}
		if matches!(self.descriptor.master_crc, Checksum::Crc32(_))
			&& declared.len() != tables.len()
		{
			return Err(Error::Unsupported(
				"master checksum with absent table checksums",
			));
		}
		let master = compare(
			self.descriptor.master_crc,
			checksum::master_crc(declared),
			|| "master image".into(),
		)?;
		check_cancellation()?;
		Ok(Verification {
			data_fork,
			tables,
			master,
		})
	}

	fn read_run(&mut self, index: usize, position: u64, output: &mut [u8]) -> Result<(), Error> {
		let run = &self.runs[index];
		let within = position - run.logical.start;
		match run.kind {
			Kind::Raw => self
				.source
				.read_exact_at(run.physical.start + within, output),
			Kind::Zero | Kind::Ignore => {
				output.fill(0);
				Ok(())
			}
			Kind::Zlib | Kind::Lzfse | Kind::Lzma2 => self.read_compressed(index, within, output),
			Kind::Unsupported(kind) => Err(Error::UnsupportedRun {
				kind,
				sector: run.logical.start / SECTOR_BYTES,
			}),
			Kind::Comment | Kind::End => {
				Err(Error::Corrupt("control record entered logical run index"))
			}
		}
	}
}

mod compressed;

fn range_len(range: &Range<u64>) -> u64 {
	range.end - range.start
}

fn compare(
	checksum: Checksum,
	actual: u32,
	domain: impl FnOnce() -> String,
) -> Result<CheckStatus, Error> {
	checksum.compare(actual, domain)?;
	Ok(match checksum {
		Checksum::Absent => CheckStatus::NotPresent,
		Checksum::Crc32(_) | Checksum::Unsupported { .. } => CheckStatus::Passed,
	})
}
