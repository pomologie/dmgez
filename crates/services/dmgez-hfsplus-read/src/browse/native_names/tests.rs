use dmgez_vfs::volume::{LookupName, NativeNameProfile};
use hfsplus_core::names::{MAX_NAME_UNITS, NameCollation, NameError};

use super::{Error, key, profile};

const CAPITALIZED: &str = "CaSe";
const LOWERCASE: &str = "case";
const COMPOSED: &str = "\u{00e9}";
const DECOMPOSED: &str = "e\u{0301}";

#[test]
fn keys_follow_the_admitted_catalog_comparator() {
	for (collation, expected) in [
		(
			NameCollation::CaseFolded,
			NativeNameProfile::HfsUnicodeFolded,
		),
		(NameCollation::Binary, NativeNameProfile::HfsXBinary),
	] {
		assert_eq!(profile(collation), Some(expected));
		for (left, right) in [(CAPITALIZED, LOWERCASE), (COMPOSED, DECOMPOSED)] {
			let left_units: Vec<_> = left.encode_utf16().collect();
			let right_units: Vec<_> = right.encode_utf16().collect();
			let left = key(collation, LookupName::Utf8(left)).unwrap().unwrap();
			let right = key(collation, LookupName::StoredUtf16(&right_units))
				.unwrap()
				.unwrap();
			assert_eq!(
				left.cmp(&right),
				collation.compare(&left_units, &right_units)
			);
		}
	}
}

#[test]
fn overlong_utf8_input_reports_the_engine_name_limit() {
	let name = LOWERCASE.repeat(MAX_NAME_UNITS / LOWERCASE.len() + 1);
	assert!(matches!(
		key(NameCollation::Binary, LookupName::Utf8(&name)),
		Err(Error::NativeName(NameError::TooLong))
	));
}
