use crate::{SECTOR_BYTES, blkx::Kind};

use super::{Run, order_runs};

const DATA_FORK_BYTES: u64 = 8;

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
