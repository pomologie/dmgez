use std::ops::Range;

use crate::blkx::Kind;

use super::{ReadCursor, Run, read_output_range};

const MAX_RUNS: usize = 8;

struct Partition {
	runs: [Run; MAX_RUNS],
	order: [usize; MAX_RUNS],
	ends: [u64; MAX_RUNS],
	count: usize,
	disk_len: u64,
}

impl Partition {
	fn symbolic() -> Self {
		let count: usize = kani::any();
		kani::assume(count <= MAX_RUNS);
		let ends: [u64; MAX_RUNS] = kani::any();
		let mut runs: [Run; MAX_RUNS] = std::array::from_fn(|_| Run {
			logical: 0..0,
			physical: 0..0,
			kind: Kind::Zero,
		});
		let mut order = [0; MAX_RUNS];
		let mut start = 0;
		for (ordinal, &end) in ends[..count].iter().enumerate() {
			kani::assume(start < end);
			let slot = MAX_RUNS - ordinal - 1;
			runs[slot].logical = start..end;
			order[ordinal] = slot;
			start = end;
		}
		Self {
			runs,
			order,
			ends,
			count,
			disk_len: start,
		}
	}

	fn order(&self) -> &[usize] {
		&self.order[..self.count]
	}

	fn admitted_request(&self) -> Range<u64> {
		let (start, end): (u64, u64) = kani::any();
		kani::assume(start <= end && end <= self.disk_len);
		start..end
	}
}

#[kani::proof]
#[kani::unwind(10)]
fn indexed_read_cursor_starts_at_containing_run() {
	let partition = Partition::symbolic();
	let request = partition.admitted_request();
	let cursor = ReadCursor::new(&partition.runs, partition.order(), request.clone());
	let ordinal = cursor.ordinal;
	assert!(ordinal <= partition.count);
	// With strictly ordered ends, these adjacent bounds uniquely identify
	// the first run whose end is after the request start.
	if ordinal > 0 {
		assert!(partition.ends[ordinal - 1] <= request.start);
	}
	if ordinal < partition.count {
		assert!(request.start < partition.ends[ordinal]);
	}
	assert_eq!(cursor.remaining, request);
	kani::cover!(ordinal == partition.count && partition.count > 0);
	kani::cover!(ordinal > 0 && ordinal < partition.count);
}

#[kani::proof]
#[kani::unwind(10)]
fn indexed_read_cursor_advances_one_exact_intersection() {
	let partition = Partition::symbolic();
	let remaining = partition.admitted_request();
	let ordinal: usize = kani::any();
	kani::assume(ordinal <= partition.count);
	if !remaining.is_empty() {
		kani::assume(ordinal < partition.count);
		let start = if ordinal == 0 {
			0
		} else {
			partition.ends[ordinal - 1]
		};
		kani::assume(start <= remaining.start && remaining.start < partition.ends[ordinal]);
	}

	let mut cursor = ReadCursor {
		remaining: remaining.clone(),
		ordinal,
	};
	let actual = cursor.next(&partition.runs, partition.order());
	if remaining.is_empty() {
		assert!(actual.is_none());
		assert_eq!(cursor.remaining, remaining);
		assert_eq!(cursor.ordinal, ordinal);
		kani::cover!(ordinal == partition.count && partition.count > 0);
		return;
	}

	let run_end = partition.ends[ordinal];
	let intersection_end = if remaining.end <= run_end {
		remaining.end
	} else {
		run_end
	};
	let (index, piece) = actual.unwrap();
	assert_eq!(index, MAX_RUNS - ordinal - 1);
	assert_eq!(piece, remaining.start..intersection_end);
	assert_eq!(cursor.remaining, intersection_end..remaining.end);
	assert_eq!(cursor.ordinal, ordinal + 1);
	if intersection_end < remaining.end {
		assert!(cursor.ordinal < partition.count);
		assert_eq!(cursor.remaining.start, partition.ends[cursor.ordinal - 1]);
		assert!(cursor.remaining.start < partition.ends[cursor.ordinal]);
		kani::cover!(remaining.start < run_end && run_end < remaining.end);
	}
}

#[kani::proof]
fn indexed_read_output_window_matches_widened_endpoints() {
	let (piece_start, piece_end, request_start): (u64, u64, u64) = kani::any();
	let output_len: usize = kani::any();
	let requested_end = u128::from(request_start) + output_len as u128;
	let admitted = request_start <= piece_start
		&& piece_start <= piece_end
		&& u128::from(piece_end) <= requested_end;

	let actual = read_output_range(&(piece_start..piece_end), request_start, output_len);
	assert_eq!(actual.is_some(), admitted);
	if let Some(window) = actual {
		assert!(window.start <= window.end && window.end <= output_len);
		assert_eq!(
			u128::from(request_start) + window.start as u128,
			u128::from(piece_start)
		);
		assert_eq!(
			u128::from(request_start) + window.end as u128,
			u128::from(piece_end)
		);
		assert_eq!(
			(window.end - window.start) as u128,
			u128::from(piece_end - piece_start)
		);
	}
	kani::cover!(admitted && request_start < piece_start && u128::from(piece_end) == requested_end);
	kani::cover!(request_start > piece_start);
	kani::cover!(piece_start <= piece_end && u128::from(piece_end) > requested_end);
}
