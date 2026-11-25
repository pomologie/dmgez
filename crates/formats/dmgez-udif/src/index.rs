use std::ops::Range;

use crate::{
	Error, SECTOR_BYTES,
	blkx::{Kind, NO_RUN_COMMENT, Record, Table},
	validation::checked_range,
};

#[cfg(test)]
#[path = "../verification/run_index_properties.rs"]
mod run_index_properties;

#[cfg(kani)]
#[path = "../verification/kani/run_admission.rs"]
mod run_admission_proof;

#[cfg(kani)]
#[path = "../verification/kani/read_ranges.rs"]
mod read_ranges_proof;

#[derive(Debug)]
pub(crate) struct Run {
	pub logical: Range<u64>,
	pub physical: Range<u64>,
	pub kind: Kind,
}

pub(crate) fn normalize(
	table: &Table,
	disk_len: u64,
	data: &Range<u64>,
) -> Result<Vec<Run>, Error> {
	let mut admission = TableAdmission::new(
		table.first_sector,
		table.sector_count,
		table.base,
		disk_len,
		data,
	)?;
	let Some((end, records)) = table.records.split_last() else {
		return Err(Error::Corrupt("mish has no terminator"));
	};
	admission.check_terminator(end)?;
	let mut runs = Vec::with_capacity(records.len());
	for record in records {
		if let Some(run) = admission.admit(record)? {
			runs.push(run);
		}
	}
	admission.finish()?;
	Ok(runs)
}

struct TableAdmission {
	logical_start: u64,
	sector_count: u64,
	base: u64,
	data_start: u64,
	data_len: u64,
	next_sector: u64,
}

enum AdmissionError {
	Corrupt(&'static str),
	Unsupported(&'static str),
}

impl From<AdmissionError> for Error {
	fn from(error: AdmissionError) -> Self {
		match error {
			AdmissionError::Corrupt(reason) => Self::Corrupt(reason),
			AdmissionError::Unsupported(reason) => Self::Unsupported(reason),
		}
	}
}

impl TableAdmission {
	fn new(
		first_sector: u64,
		sector_count: u64,
		base: u64,
		disk_len: u64,
		data: &Range<u64>,
	) -> Result<Self, AdmissionError> {
		let logical = checked_range(
			sector_bytes(first_sector)?,
			sector_bytes(sector_count)?,
			disk_len,
		)
		.filter(|range| !range.is_empty())
		.ok_or(AdmissionError::Corrupt("mish logical range outside disk"))?;
		Ok(Self {
			logical_start: logical.start,
			sector_count,
			base,
			data_start: data.start,
			data_len: data.end - data.start,
			next_sector: 0,
		})
	}

	fn check_terminator(&self, end: &Record) -> Result<(), AdmissionError> {
		if end.kind != Kind::End
			|| end.first_sector != self.sector_count
			|| end.sector_count != 0
			|| end.stored_len != 0
			|| end.comment != NO_RUN_COMMENT
		{
			return Err(AdmissionError::Corrupt("invalid mish terminator"));
		}
		let terminal_offset = self
			.base
			.checked_add(end.offset)
			.ok_or(AdmissionError::Corrupt("mish terminator offset overflow"))?;
		if terminal_offset > self.data_len {
			return Err(AdmissionError::Corrupt(
				"mish terminator offset outside the data fork",
			));
		}
		Ok(())
	}

	fn admit(&mut self, record: &Record) -> Result<Option<Run>, AdmissionError> {
		let offset = self
			.base
			.checked_add(record.offset)
			.ok_or(AdmissionError::Corrupt("mish data offset overflow"))?;
		let physical = checked_range(offset, record.stored_len, self.data_len)
			.ok_or(AdmissionError::Corrupt("mish run outside the data fork"))?;
		match record.kind {
			Kind::End => return Err(AdmissionError::Corrupt("early mish terminator")),
			Kind::Comment => {
				if record.sector_count != 0
					|| record.stored_len != 0
					|| record.first_sector > self.sector_count
				{
					return Err(AdmissionError::Corrupt("comment run describes image data"));
				}
				return Ok(None);
			}
			_ => {}
		}
		if record.comment != NO_RUN_COMMENT {
			return Err(AdmissionError::Unsupported("nonzero data-run flags"));
		}
		if record.first_sector != self.next_sector || record.sector_count == 0 {
			return Err(AdmissionError::Corrupt(
				"misordered, overlapping or empty mish run",
			));
		}
		let sectors = checked_range(record.first_sector, record.sector_count, self.sector_count)
			.ok_or(AdmissionError::Corrupt(
				"mish run outside its logical table",
			))?;
		let decoded_len = sector_bytes(record.sector_count)?;
		match record.kind {
			Kind::Raw if record.stored_len != decoded_len => {
				return Err(AdmissionError::Corrupt(
					"raw run length differs from its sector count",
				));
			}
			Kind::Zlib if physical.is_empty() => {
				return Err(AdmissionError::Corrupt("empty zlib run"));
			}
			Kind::Lzfse if physical.is_empty() => {
				return Err(AdmissionError::Corrupt("empty LZFSE run"));
			}
			Kind::Lzma2 if physical.is_empty() => {
				return Err(AdmissionError::Corrupt("empty LZMA2 run"));
			}
			Kind::Zero | Kind::Ignore if !physical.is_empty() => {
				return Err(AdmissionError::Corrupt(
					"zero or ignore run has physical data",
				));
			}
			_ => {}
		}
		let run = Run {
			logical: self.logical_start + sector_bytes(sectors.start)?
				..self.logical_start + sector_bytes(sectors.end)?,
			physical: self.data_start + physical.start..self.data_start + physical.end,
			kind: record.kind,
		};
		self.next_sector = sectors.end;
		Ok(Some(run))
	}

	fn finish(self) -> Result<(), AdmissionError> {
		if self.next_sector != self.sector_count {
			return Err(AdmissionError::Corrupt(
				"mish runs do not cover their table",
			));
		}
		Ok(())
	}
}

pub(crate) fn order_runs(runs: &[Run], disk_len: u64) -> Result<Vec<usize>, Error> {
	let mut ordered: Vec<_> = (0..runs.len()).collect();
	ordered.sort_unstable_by_key(|&i| runs[i].logical.start);
	let mut expected = 0;
	for &index in &ordered {
		let run = &runs[index];
		if run.logical.start != expected {
			return Err(Error::Corrupt("block maps have a logical gap or overlap"));
		}
		expected = run.logical.end;
	}
	if expected != disk_len {
		return Err(Error::Corrupt("block maps do not cover the logical disk"));
	}
	let mut physical: Vec<_> = runs.iter().filter(|run| !run.physical.is_empty()).collect();
	physical.sort_unstable_by_key(|run| run.physical.start);
	if physical
		.windows(2)
		.any(|pair| pair[0].physical.end > pair[1].physical.start)
	{
		return Err(Error::Corrupt("mish runs reuse overlapping physical data"));
	}
	Ok(ordered)
}

// A cursor over a request admitted against a contiguous logical partition.
pub(crate) struct ReadCursor {
	remaining: Range<u64>,
	ordinal: usize,
}

impl ReadCursor {
	pub(crate) fn new(runs: &[Run], logical_order: &[usize], request: Range<u64>) -> Self {
		let ordinal =
			logical_order.partition_point(|&index| runs[index].logical.end <= request.start);
		Self {
			remaining: request,
			ordinal,
		}
	}

	pub(crate) fn next(
		&mut self,
		runs: &[Run],
		logical_order: &[usize],
	) -> Option<(usize, Range<u64>)> {
		if self.remaining.is_empty() {
			return None;
		}
		let index = logical_order[self.ordinal];
		let end = runs[index].logical.end.min(self.remaining.end);
		let piece = self.remaining.start..end;
		self.remaining.start = end;
		self.ordinal += 1;
		Some((index, piece))
	}
}

pub(crate) fn bytes(sectors: u64) -> Result<u64, Error> {
	sector_bytes(sectors).map_err(Error::from)
}

fn sector_bytes(sectors: u64) -> Result<u64, AdmissionError> {
	sectors
		.checked_mul(SECTOR_BYTES)
		.ok_or(AdmissionError::Corrupt(
			"sector count overflows byte geometry",
		))
}
