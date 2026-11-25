use super::*;

#[test]
fn keeps_case_distinct_paths_and_rejects_wrong_case_targets() {
	let mut image = snapshot();
	image.format = VolumeFormat::HfsX;
	let input = "/Right/shortcut";
	let host_file = captured_file(input, b"separate host file");
	apply_with_copies(&mut image, &[copy(input)], vec![host_file]).unwrap();
	assert!(image.entries.contains_key(&path(input)));
	assert_eq!(target(&image, "/Right/Shortcut"), b"other");
	assert!(matches!(
		apply(&mut image, &[remove("/Right/SHORTCUT")], Path::new(".")),
		Err(OverlayError::UnknownTarget { index: 0, .. })
	));
	assert!(matches!(
		apply(&mut image, &[rename(input, "/right/Moved")], Path::new(".")),
		Err(OverlayError::Parent { .. })
	));
	apply(&mut image, &[remove(input)], Path::new(".")).unwrap();
	assert!(!image.entries.contains_key(&path(input)));
	assert_eq!(target(&image, "/Right/Shortcut"), b"other");
}

#[test]
fn still_resolves_and_rejects_colliding_canonical_spellings() {
	let mut image = snapshot();
	image.format = VolumeFormat::HfsX;
	assert!(matches!(
		apply(&mut image, &[add_directory("/Left/É")], Path::new(".")),
		Err(OverlayError::Collision { .. })
	));
	apply(
		&mut image,
		&[rename("/Left/É", "/Left/Renamed")],
		Path::new("."),
	)
	.unwrap();
	assert_eq!(target(&image, "/Left/Renamed"), b"hard\xfftarget");
}

#[test]
fn preserves_exact_finder_record_ownership_and_store_name_matching() {
	let mut image = snapshot();
	image.format = VolumeFormat::HfsX;
	add_store(&mut image, "/Right/.DS_Store", 31, &[("Shortcut", 1, 2)]);
	let lower_store = "/Right/.ds_store";
	let host_file = captured_file(lower_store, b"ordinary file");
	apply_with_copies(&mut image, &[copy(lower_store)], vec![host_file]).unwrap();
	assert!(matches!(
		apply(&mut image, &[remove("/Right/.DS_Store")], Path::new(".")),
		Err(OverlayError::FinderStore { .. })
	));
	assert_eq!(locations(&image, "/Right/.DS_Store"), [("Shortcut", 1, 2)]);
	apply(
		&mut image,
		&[
			rename("/Right/Shortcut", "/Right/shortcut"),
			remove(lower_store),
		],
		Path::new("."),
	)
	.unwrap();
	assert!(!image.entries.contains_key(&path(lower_store)));
	assert_eq!(locations(&image, "/Right/.DS_Store"), [("shortcut", 1, 2)]);
}

#[test]
fn refuses_finder_locations_that_the_store_cannot_distinguish() {
	let mut image = snapshot();
	image.format = VolumeFormat::HfsX;
	let input = "/Right/shortcut";
	apply_with_copies(&mut image, &[copy(input)], vec![captured_file(input, b"")]).unwrap();
	add_store(&mut image, "/Right/.DS_Store", 31, &[("Shortcut", 1, 2)]);
	let original = store_bytes(&image, "/Right/.DS_Store");
	assert!(matches!(
		apply(
			&mut image,
			&[position(input, FieldChange::Set([3.0, 4.0]))],
			Path::new(".")
		),
		Err(OverlayError::FinderLayout {
			source: StoreEditError::DuplicateName(_),
			..
		})
	));
	assert_eq!(store_bytes(&image, "/Right/.DS_Store"), original);
}
