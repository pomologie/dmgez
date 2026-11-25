use super::{BackgroundAliasName, HfsCreationTime, components, matches_date, matches_name};

#[test]
fn alias_paths_preserve_exact_components_and_reject_ambiguous_paths() {
	let path = "/.background/Cafe\u{301}.png";
	let parts = components(Some(path.as_bytes())).unwrap();
	assert_eq!(
		parts,
		[".background", "Cafe\u{301}.png"].map(|part| part.encode_utf16().collect::<Vec<_>>())
	);
	for path in [
		".background/asset.png",
		"/.background/../asset.png",
		"/.background/./asset.png",
		"/.background//asset.png",
		"/.background/asset.png/",
		"/.background/C:\\asset.png",
		"/.background/ass\0et.png",
		"/",
	] {
		assert!(components(Some(path.as_bytes())).is_none(), "{path:?}");
	}
}

#[test]
fn alias_names_and_creation_dates_must_match_exactly() {
	let stored = "Cafe\u{301}".encode_utf16().collect::<Vec<_>>();
	let alias = "Café";
	assert!(!matches_name(
		Some(&BackgroundAliasName::Unicode(alias.into())),
		&stored
	));
	let exact = "Cafe\u{301}";
	assert!(matches_name(
		Some(&BackgroundAliasName::Unicode(exact.into())),
		&stored
	));
	assert!(!matches_name(
		Some(&BackgroundAliasName::Legacy(vec![0x8e])),
		&[0xe9]
	));
	let seconds = 3_717_785_600;
	assert!(matches_date(HfsCreationTime::Seconds(seconds), seconds));
	assert!(!matches_date(
		HfsCreationTime::Seconds(seconds - 1),
		seconds
	));
	let fixed = u64::from(seconds) << 16;
	assert!(matches_date(HfsCreationTime::FixedPoint(fixed), seconds));
	assert!(!matches_date(
		HfsCreationTime::FixedPoint(fixed + 1),
		seconds
	));
}
