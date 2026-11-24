use std::ops::Range;

use crate::{
	Error, SECTOR_BYTES,
	blkx::{Kind, NO_RUN_COMMENT, Table},
	validation::checked_range,
};

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
	let logical = checked_range(
		bytes(table.first_sector)?,
		bytes(table.sector_count)?,
		disk_len,
	)
	.filter(|range| !range.is_empty())
	.ok_or(Error::Corrupt("mish logical range outside disk"))?;
	let data_len = data.end - data.start;
	let Some((end, records)) = table.records.split_last() else {
		return Err(Error::Corrupt("mish has no terminator"));
	};
	if end.kind != Kind::End
		|| end.first_sector != table.sector_count
		|| end.sector_count != 0
		|| end.stored_len != 0
		|| end.comment != NO_RUN_COMMENT
	{
		return Err(Error::Corrupt("invalid mish terminator"));
	}
	let terminal_offset = table
		.base
		.checked_add(end.offset)
		.ok_or(Error::Corrupt("mish terminator offset overflow"))?;
	if terminal_offset > data_len {
		return Err(Error::Corrupt(
			"mish terminator offset outside the data fork",
		));
	}
	let mut runs = Vec::with_capacity(records.len());
	let mut expected_sector = 0;
	for record in records {
		let offset = table
			.base
			.checked_add(record.offset)
			.ok_or(Error::Corrupt("mish data offset overflow"))?;
		let physical = checked_range(offset, record.stored_len, data_len)
			.ok_or(Error::Corrupt("mish run outside the data fork"))?;
		match record.kind {
			Kind::End => return Err(Error::Corrupt("early mish terminator")),
			Kind::Comment => {
				if record.sector_count != 0
					|| record.stored_len != 0
					|| record.first_sector > table.sector_count
				{
					return Err(Error::Corrupt("comment run describes image data"));
				}
				continue;
			}
			_ => {}
		}
		if record.comment != NO_RUN_COMMENT {
			return Err(Error::Unsupported("nonzero data-run flags"));
		}
		if record.first_sector != expected_sector || record.sector_count == 0 {
			return Err(Error::Corrupt("misordered, overlapping or empty mish run"));
		}
		let sectors = checked_range(record.first_sector, record.sector_count, table.sector_count)
			.ok_or(Error::Corrupt("mish run outside its logical table"))?;
		expected_sector = sectors.end;
		let decoded_len = bytes(record.sector_count)?;
		match record.kind {
			Kind::Raw if record.stored_len != decoded_len => {
				return Err(Error::Corrupt(
					"raw run length differs from its sector count",
				));
			}
			Kind::Zlib if physical.is_empty() => return Err(Error::Corrupt("empty zlib run")),
			Kind::Lzfse if physical.is_empty() => return Err(Error::Corrupt("empty LZFSE run")),
			Kind::Zero | Kind::Ignore if !physical.is_empty() => {
				return Err(Error::Corrupt("zero or ignore run has physical data"));
			}
			_ => {}
		}
		runs.push(Run {
			logical: logical.start + bytes(sectors.start)?..logical.start + bytes(sectors.end)?,
			physical: data.start + physical.start..data.start + physical.end,
			kind: record.kind,
		});
	}
	if expected_sector != table.sector_count {
		return Err(Error::Corrupt("mish runs do not cover their table"));
	}
	Ok(runs)
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
	sectors
		.checked_mul(SECTOR_BYTES)
		.ok_or(Error::Corrupt("sector count overflows byte geometry"))
}
