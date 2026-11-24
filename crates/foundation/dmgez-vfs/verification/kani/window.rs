use crate::window::translate;

/// Byte translations match widened arithmetic over up to four admitted windows.
#[kani::proof]
#[kani::unwind(5)]
fn partition_subwindows_match_widened_geometry() {
	let start: u64 = kani::any();
	let end: u64 = kani::any();
	kani::assume(start <= end);
	let mut parent = start..end;
	for _ in 0..4 {
		let offset: u64 = kani::any();
		let len: u64 = kani::any();
		let expected_start = u128::from(parent.start) + u128::from(offset);
		let expected_end = expected_start + u128::from(len);
		let result = translate(&parent, offset, len);
		assert_eq!(result.is_some(), expected_end <= u128::from(parent.end));
		let Some(child) = result else {
			return;
		};
		assert_eq!(u128::from(child.start), expected_start);
		assert_eq!(u128::from(child.end), expected_end);
		assert!(start <= child.start && child.end <= end);
		parent = child;
	}
}
