use std::ops::Range;

use crate::{
	Error, SECTOR_BYTES,
	blkx::{Kind, NO_RUN_COMMENT, Record, Table},
	checksum::Checksum,
};

use super::{Run, normalize, order_runs, read_output_range};

const DATA_FORK_BYTES: u64 = 8;
const BASE: u64 = 1;
const FORK: Range<u64> = 2 * SECTOR_BYTES..5 * SECTOR_BYTES;
const FORK_LEN: u64 = FORK.end - FORK.start;
const TWO_SECTORS: u64 = 2 * SECTOR_BYTES;
const NONZERO_FLAGS: u32 = 1;
const RAW: Record = Record {
	kind: Kind::Raw,
	comment: NO_RUN_COMMENT,
	first_sector: 0,
	sector_count: 1,
	offset: 0,
	stored_len: SECTOR_BYTES,
};
const END: Record = Record {
	kind: Kind::End,
	comment: NO_RUN_COMMENT,
	first_sector: 1,
	sector_count: 0,
	offset: SECTOR_BYTES,
	stored_len: 0,
};

fn table(records: impl Into<Vec<Record>>) -> Table {
	Table {
		first_sector: 0,
		sector_count: 1,
		base: BASE,
		checksum: Checksum::Absent,
		buffer_sectors: 0,
		descriptor: 0,
		records: records.into(),
	}
}

fn rejects_table(table: &Table, reason: &str) {
	let error = normalize(table, TWO_SECTORS, &FORK).unwrap_err();
	assert!(
		matches!(error, Error::Corrupt(actual) if actual == reason),
		"{error:?}"
	);
}

#[test]
fn admits_exact_table_and_fork_bounds() {
	let mut table = table([RAW, END]);
	table.first_sector = 1;
	table.base = FORK_LEN - SECTOR_BYTES;
	let runs = normalize(&table, TWO_SECTORS, &FORK).unwrap();
	let [run] = runs.as_slice() else {
		panic!("one data record must project exactly one run");
	};
	assert_eq!(run.logical, SECTOR_BYTES..TWO_SECTORS);
	assert_eq!(run.physical, FORK.end - SECTOR_BYTES..FORK.end);
	assert_eq!(run.kind, Kind::Raw);
}

#[test]
fn rejects_invalid_logical_table_bounds_before_processing_records() {
	let mut table = table([RAW, END]);
	for (first_sector, sector_count, reason) in [
		(0, 0, "mish logical range outside disk"),
		(2, 1, "mish logical range outside disk"),
		(u64::MAX, 1, "sector count overflows byte geometry"),
		(0, u64::MAX, "sector count overflows byte geometry"),
	] {
		table.first_sector = first_sector;
		table.sector_count = sector_count;
		rejects_table(&table, reason);
	}
}

#[test]
fn rejects_malformed_and_out_of_fork_terminators() {
	for end in [
		Record {
			kind: Kind::Zero,
			..END
		},
		Record {
			first_sector: 0,
			..END
		},
		Record {
			sector_count: 1,
			..END
		},
		Record {
			stored_len: 1,
			..END
		},
		Record {
			comment: NONZERO_FLAGS,
			..END
		},
	] {
		rejects_table(&table([RAW, end]), "invalid mish terminator");
	}
	rejects_table(
		&table([
			RAW,
			Record {
				offset: u64::MAX,
				..END
			},
		]),
		"mish terminator offset overflow",
	);
	rejects_table(
		&table([
			RAW,
			Record {
				offset: FORK_LEN,
				..END
			},
		]),
		"mish terminator offset outside the data fork",
	);
	rejects_table(&table([END, END]), "early mish terminator");
}

#[test]
fn rejects_comment_records_that_claim_payload_or_out_of_table_positions() {
	let comment = Record {
		kind: Kind::Comment,
		first_sector: 0,
		offset: 0,
		..END
	};
	for invalid in [
		Record {
			sector_count: 1,
			..comment
		},
		Record {
			stored_len: 1,
			..comment
		},
		Record {
			first_sector: 2,
			..comment
		},
	] {
		rejects_table(
			&table([invalid, RAW, END]),
			"comment run describes image data",
		);
	}
}

#[test]
fn rejects_invalid_data_record_geometry_and_unknown_flags() {
	for (record, reason) in [
		(
			Record {
				offset: u64::MAX,
				..RAW
			},
			"mish data offset overflow",
		),
		(
			Record {
				offset: FORK_LEN,
				..RAW
			},
			"mish run outside the data fork",
		),
		(
			Record {
				sector_count: 0,
				..RAW
			},
			"misordered, overlapping or empty mish run",
		),
		(
			Record {
				sector_count: 2,
				..RAW
			},
			"mish run outside its logical table",
		),
		(
			Record {
				stored_len: SECTOR_BYTES - 1,
				..RAW
			},
			"raw run length differs from its sector count",
		),
	] {
		rejects_table(&table([record, END]), reason);
	}
	let flagged = table([
		Record {
			comment: NONZERO_FLAGS,
			..RAW
		},
		END,
	]);
	assert!(matches!(
		normalize(&flagged, TWO_SECTORS, &FORK),
		Err(Error::Unsupported("nonzero data-run flags"))
	));
}

#[test]
fn rejects_payload_on_zero_or_ignore_and_missing_compressed_payload() {
	for kind in [Kind::Zero, Kind::Ignore] {
		rejects_table(
			&table([Record { kind, ..RAW }, END]),
			"zero or ignore run has physical data",
		);
	}
	for (kind, reason) in [
		(Kind::Zlib, "empty zlib run"),
		(Kind::Lzfse, "empty LZFSE run"),
		(Kind::Lzma2, "empty LZMA2 run"),
	] {
		let record = Record {
			kind,
			stored_len: 0,
			..RAW
		};
		rejects_table(&table([record, END]), reason);
	}
}

#[test]
fn rejects_incomplete_logical_runs_within_and_across_tables() {
	rejects_table(&table([END]), "mish runs do not cover their table");

	let first = normalize(&table([RAW, END]), TWO_SECTORS, &FORK).unwrap();
	assert!(matches!(
		order_runs(&first, TWO_SECTORS),
		Err(Error::Corrupt("block maps do not cover the logical disk"))
	));
	let mut last = table([RAW, END]);
	last.first_sector = 1;
	let last = normalize(&last, TWO_SECTORS, &FORK).unwrap();
	assert!(matches!(
		order_runs(&last, TWO_SECTORS),
		Err(Error::Corrupt("block maps have a logical gap or overlap"))
	));
}

#[test]
fn read_output_window_rejects_reversed_and_out_of_buffer_fragments() {
	const OUTPUT_BYTES: usize = 8;
	let request_end = u64::MAX;
	let request_start = request_end - OUTPUT_BYTES as u64;
	let whole_request = request_start..request_end;
	assert_eq!(
		read_output_range(&whole_request, request_start, OUTPUT_BYTES),
		Some(0..OUTPUT_BYTES)
	);
	let empty_at_end = request_end..request_end;
	assert_eq!(
		read_output_range(&empty_at_end, request_start, OUTPUT_BYTES),
		Some(OUTPUT_BYTES..OUTPUT_BYTES)
	);

	let reversed = request_end..request_start;
	let before_request = request_start - 1..request_end;
	assert_eq!(
		read_output_range(&reversed, request_start, OUTPUT_BYTES),
		None
	);
	assert_eq!(
		read_output_range(&before_request, request_start, OUTPUT_BYTES),
		None
	);
	assert_eq!(
		read_output_range(&whole_request, request_start, OUTPUT_BYTES - 1),
		None
	);
}

#[test]
fn two_tables_admit_only_disjoint_physical_bytes() {
	let spans: Vec<_> = (0..=DATA_FORK_BYTES)
		.flat_map(|start| (start..=DATA_FORK_BYTES).map(move |end| start..end))
		.collect();
	for left in &spans {
		for right in &spans {
			let runs = [
				Run {
					logical: SECTOR_BYTES..2 * SECTOR_BYTES,
					physical: right.clone(),
					kind: if right.is_empty() {
						Kind::Zero
					} else {
						Kind::Zlib
					},
				},
				Run {
					logical: 0..SECTOR_BYTES,
					physical: left.clone(),
					kind: if left.is_empty() {
						Kind::Ignore
					} else {
						Kind::Zlib
					},
				},
			];
			let disjoint = left.clone().all(|byte| !right.contains(&byte));
			let result = order_runs(&runs, 2 * SECTOR_BYTES);
			assert_eq!(result.is_ok(), disjoint, "{left:?}, {right:?}");
			if let Ok(order) = result {
				assert_eq!(order, [1, 0]);
			}
		}
	}
}
