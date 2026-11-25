use std::ops::Range;

use crate::{
	SECTOR_BYTES,
	blkx::{Kind, NO_RUN_COMMENT, Record},
};

use super::{Run, TableAdmission, order_runs};

const MAX_RUNS: u8 = 8;
const MAX_SECTORS: u8 = 16;
const CLASSIC_READ_KINDS: [Kind; 6] = [
	Kind::Zero,
	Kind::Raw,
	Kind::Ignore,
	Kind::Zlib,
	Kind::Comment,
	Kind::End,
];

fn sector() -> u64 {
	let value: u8 = kani::any();
	kani::assume(value <= MAX_SECTORS);
	u64::from(value)
}

fn zero_run(first_sector: u64, sector_count: u64) -> Record {
	Record {
		kind: Kind::Zero,
		comment: NO_RUN_COMMENT,
		first_sector,
		sector_count,
		offset: 0,
		stored_len: 0,
	}
}

fn terminator(sector_count: u64) -> Record {
	Record {
		kind: Kind::End,
		..zero_run(sector_count, 0)
	}
}

fn symbolic_record() -> Record {
	let kind: u8 = kani::any();
	kani::assume(usize::from(kind) < CLASSIC_READ_KINDS.len());
	let offset: u16 = kani::any();
	let stored_len: u16 = kani::any();
	Record {
		kind: CLASSIC_READ_KINDS[usize::from(kind)],
		comment: kani::any(),
		first_sector: sector(),
		sector_count: sector(),
		offset: u64::from(offset),
		stored_len: u64::from(stored_len),
	}
}

fn assert_exact_projection(
	run: &Run,
	record: &Record,
	first_sector: u64,
	base: u64,
	data: &Range<u64>,
) {
	let sector_start = first_sector + record.first_sector;
	let logical = sector_start * SECTOR_BYTES..(sector_start + record.sector_count) * SECTOR_BYTES;
	assert_eq!(run.kind, record.kind);
	assert_eq!(run.logical, logical);
	assert!(run.logical.start < run.logical.end);
	let physical_start = u128::from(data.start) + u128::from(base) + u128::from(record.offset);
	let physical_end = physical_start + u128::from(record.stored_len);
	assert!(physical_end <= u128::from(data.end));
	assert_eq!(u128::from(run.physical.start), physical_start);
	assert_eq!(u128::from(run.physical.end), physical_end);
	match run.kind {
		Kind::Raw => assert_eq!(record.stored_len, record.sector_count * SECTOR_BYTES),
		Kind::Zlib => assert!(record.stored_len > 0),
		Kind::Zero | Kind::Ignore => assert_eq!(record.stored_len, 0),
		_ => unreachable!("only classic logical run types can be admitted"),
	}
}

#[kani::proof]
#[kani::unwind(10)]
fn run_admission_projects_the_exact_table_partition() {
	let count: u8 = kani::any();
	kani::assume(count <= MAX_RUNS);
	let first_sector = sector();
	let sector_count = sector();
	let disk_sectors = sector();
	let probe_sector = sector();
	let terminal: u16 = kani::any();
	let terminal_record = Record {
		offset: u64::from(terminal),
		..terminator(sector_count)
	};
	let (data_start, data_end, base): (u64, u64, u64) = kani::any();
	kani::assume(data_start <= data_end);
	let data = data_start..data_end;
	let Ok(mut admission) = TableAdmission::new(
		first_sector,
		sector_count,
		base,
		disk_sectors * SECTOR_BYTES,
		&data,
	) else {
		return;
	};
	if admission.check_terminator(&terminal_record).is_err() {
		return;
	}
	assert!(sector_count > 0 && first_sector + sector_count <= disk_sectors);
	assert!(u128::from(base) + u128::from(terminal) <= u128::from(data_end - data_start));

	let mut projected = 0;
	let mut previous_end = first_sector * SECTOR_BYTES;
	let mut coverage = 0;
	let mut has_ignore = false;
	let mut has_zlib = false;
	for _ in 0..count {
		let record = symbolic_record();
		let Ok(run) = admission.admit(&record) else {
			return;
		};
		let physical_end =
			u128::from(base) + u128::from(record.offset) + u128::from(record.stored_len);
		assert!(physical_end <= u128::from(data_end - data_start));
		let Some(run) = run else {
			assert_eq!(record.kind, Kind::Comment);
			assert_eq!(record.sector_count, 0);
			assert_eq!(record.stored_len, 0);
			assert!(record.first_sector <= sector_count);
			continue;
		};
		assert_eq!(record.comment, NO_RUN_COMMENT);
		projected += 1;
		assert_exact_projection(&run, &record, first_sector, base, &data);
		assert_eq!(run.logical.start, previous_end);
		previous_end = run.logical.end;
		has_ignore |= run.kind == Kind::Ignore;
		has_zlib |= run.kind == Kind::Zlib;
		if run.logical.contains(&(probe_sector * SECTOR_BYTES)) {
			coverage += 1;
		}
	}
	if admission.finish().is_err() {
		return;
	}
	assert_eq!(previous_end, (first_sector + sector_count) * SECTOR_BYTES);
	let in_table = (first_sector..first_sector + sector_count).contains(&probe_sector);
	assert_eq!(coverage, usize::from(in_table));
	kani::cover!(projected == MAX_RUNS && in_table);
	kani::cover!(has_ignore);
	kani::cover!(has_zlib);
	kani::cover!(projected < count);
}

#[kani::proof]
#[kani::unwind(5)]
fn run_admission_checks_full_width_physical_bounds() {
	let (data_start, data_end, base, offset, stored_len): (u64, u64, u64, u64, u64) = kani::any();
	kani::assume(data_start <= data_end);
	let data = data_start..data_end;
	let record = Record {
		kind: Kind::Zlib,
		offset,
		stored_len,
		..zero_run(0, 1)
	};
	let Ok(mut admission) = TableAdmission::new(0, 1, base, SECTOR_BYTES, &data) else {
		unreachable!("the one-sector logical table is admissible");
	};
	let start = u128::from(data_start) + u128::from(base) + u128::from(offset);
	let end = start + u128::from(stored_len);
	let result = admission.admit(&record);
	assert_eq!(
		result.is_ok(),
		stored_len > 0 && end <= u128::from(data_end)
	);
	if let Ok(run) = result {
		let run = run.expect("a compressed run covers one admitted sector");
		assert_exact_projection(&run, &record, 0, base, &data);
		assert!(admission.finish().is_ok());
	}
	kani::cover!(end > u128::from(u64::MAX) && start <= u128::from(u64::MAX));
	kani::cover!(end == u128::from(data_end) && stored_len > 0);
}

#[kani::proof]
#[kani::unwind(4)]
fn run_admission_rejects_gaps_and_overlaps_across_tables() {
	let disk_sectors = sector();
	let left_end = sector();
	let right_start = sector();
	kani::assume(left_end > 0 && left_end <= disk_sectors && right_start < disk_sectors);
	let disk_len = disk_sectors * SECTOR_BYTES;
	// Resource order is deliberately the reverse of logical order.
	let runs = [
		Run {
			logical: right_start * SECTOR_BYTES..disk_len,
			physical: 0..0,
			kind: Kind::Zero,
		},
		Run {
			logical: 0..left_end * SECTOR_BYTES,
			physical: 0..0,
			kind: Kind::Ignore,
		},
	];
	let result = order_runs(&runs, disk_len);
	assert_eq!(result.is_ok(), left_end == right_start);
	if let Ok(order) = result {
		assert_eq!(order.len(), 2);
		assert_eq!(order[0], 1);
		assert_eq!(order[1], 0);
	}
	kani::cover!(left_end < right_start);
	kani::cover!(left_end > right_start);
	kani::cover!(left_end == right_start);
}

#[kani::proof]
#[kani::unwind(5)]
fn run_admission_requires_a_well_formed_terminator() {
	let terminal = symbolic_record();
	let data_end: u16 = kani::any();
	let Ok(admission) = TableAdmission::new(0, 1, 0, SECTOR_BYTES, &(0..u64::from(data_end)))
	else {
		unreachable!("the one-sector logical table is admissible");
	};
	let expected = terminal.kind == Kind::End
		&& terminal.first_sector == 1
		&& terminal.sector_count == 0
		&& terminal.stored_len == 0
		&& terminal.comment == NO_RUN_COMMENT
		&& terminal.offset <= u64::from(data_end);
	let result = admission.check_terminator(&terminal);
	assert_eq!(result.is_ok(), expected);
	kani::cover!(expected);
	kani::cover!(terminal.kind == Kind::End && terminal.first_sector != 1);
}
