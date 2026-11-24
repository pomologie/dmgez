use std::ops::Range;

/// Return the half-open window only when both its end and parent bound hold.
pub(crate) fn checked_range(offset: u64, len: u64, limit: u64) -> Option<Range<u64>> {
	let end = offset.checked_add(len)?;
	(end <= limit).then_some(offset..end)
}

#[cfg(test)]
mod tests {
	use proptest::prelude::*;

	use super::checked_range;

	proptest! {
		#[test]
		fn agrees_with_widened_arithmetic(offset: u64, len: u64, limit: u64) {
			let actual = checked_range(offset, len, limit);
			let end = u128::from(offset) + u128::from(len);
			prop_assert_eq!(actual.is_some(), end <= u128::from(limit));
			if let Some(range) = actual {
				prop_assert_eq!(range.start, offset);
				prop_assert_eq!(u128::from(range.end), end);
			}
		}
	}
}
