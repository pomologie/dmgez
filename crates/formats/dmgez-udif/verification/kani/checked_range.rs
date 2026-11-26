use crate::validation::checked_range;

#[kani::proof]
fn checked_range_matches_mathematical_window() {
	let (offset, len, limit): (u64, u64, u64) = kani::any();
	let end = u128::from(offset) + u128::from(len);
	let actual = checked_range(offset, len, limit);
	assert_eq!(actual.is_some(), end <= u128::from(limit));
	if let Some(range) = &actual {
		assert_eq!(range.start, offset);
		assert_eq!(u128::from(range.end), end);
	}
	kani::cover!(actual.is_some() && len > 0 && end == u128::from(limit));
	kani::cover!(actual.is_none() && end > u128::from(u64::MAX));
}
