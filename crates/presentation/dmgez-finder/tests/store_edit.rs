//! Lossless record edits for D-FINDER-03-T04.

use std::{
	io::Write,
	process::{Command, Stdio},
};

use dmgez_finder::{
	edit::{EditError, IconPositionEdit, edit_icon_positions},
	inspection::{ReadError, ReadLimits},
};
use ds_parser::{FieldKey, FourCC, IconLocation, Record, ValueTag};

const NATIVE_STORE: &[u8] = include_bytes!("fixtures/finder-macos.ds_store");
const ORACLE: &str = include_str!("store_edit_oracle.py");
const OPAQUE_FIELD: [u8; 4] = *b"ext1";
const UNSUPPORTED_TYPE: [u8; 4] = *b"unkn";
const OPAQUE_NAME: &str = "zz-opaque";
const OPAQUE_VALUE: [u8; 5] = [0x00, 0xff, 0x41, 0xa5, 0x00];
const BAM_EDIT: IconPositionEdit<'_> = IconPositionEdit {
	name: "bam",
	x: 530,
	y: 180,
};
const BAR_EDIT: IconPositionEdit<'_> = IconPositionEdit {
	name: "bar",
	x: 420,
	y: 55,
};

#[test]
fn replaces_only_selected_coordinate_bytes_in_finder_authored_store() {
	let mut actual = NATIVE_STORE.to_vec();
	edit_icon_positions(&mut actual, &[BAM_EDIT, BAR_EDIT], ReadLimits::default()).unwrap();
	let mut expected = NATIVE_STORE.to_vec();
	replace_coordinates(&mut expected, (104, 116), BAM_EDIT);
	replace_coordinates(&mut expected, (256, 235), BAR_EDIT);
	assert_eq!(actual, expected);
}

#[test]
fn retains_every_byte_of_an_unknown_record() {
	let original = store_with_opaque_record();
	let mut actual = original.clone();
	edit_icon_positions(&mut actual, &[BAM_EDIT], ReadLimits::default()).unwrap();
	let mut expected = original;
	replace_coordinates(&mut expected, (104, 116), BAM_EDIT);
	assert_eq!(actual, expected);
}

#[test]
fn refuses_missing_and_duplicate_targets_without_mutating_the_input() {
	let mut store = NATIVE_STORE.to_vec();
	let limits = ReadLimits::default();
	let wrong_case = IconPositionEdit {
		name: "BAR",
		..BAR_EDIT
	};
	assert!(matches!(
		edit_icon_positions(&mut store, &[BAM_EDIT, wrong_case], limits),
		Err(EditError::MissingPosition(name)) if name == wrong_case.name
	));
	assert_eq!(store, NATIVE_STORE);
	assert!(matches!(
		edit_icon_positions(&mut store, &[BAM_EDIT, BAM_EDIT], limits),
		Err(EditError::DuplicateTarget(name)) if name == BAM_EDIT.name
	));
	assert_eq!(store, NATIVE_STORE);
}

#[test]
fn refuses_unknown_record_lengths_without_applying_earlier_replacements() {
	let mut store = store_with_opaque_record();
	let field = unique_offset(&store, &OPAQUE_FIELD);
	let tag_offset = field + OPAQUE_FIELD.len();
	let stored_tag = &mut store[tag_offset..tag_offset + UNSUPPORTED_TYPE.len()];
	assert_eq!(stored_tag, &ValueTag::Blob.fourcc().bytes());
	stored_tag.copy_from_slice(&UNSUPPORTED_TYPE);
	let before = store.clone();
	assert!(matches!(
		edit_icon_positions(&mut store, &[BAM_EDIT], ReadLimits::default()),
		Err(EditError::Read(ReadError::UnsupportedType(
			UNSUPPORTED_TYPE
		)))
	));
	assert_eq!(store, before);
}

#[test]
fn bounds_the_edit_count_and_utf16_name_length() {
	let mut store = NATIVE_STORE.to_vec();
	let limits = ReadLimits::default();
	let name = IconPositionEdit {
		name: "file-📋",
		..BAM_EDIT
	};
	assert!(matches!(
		edit_icon_positions(
			&mut store,
			&[name],
			ReadLimits {
				name_units: name.name.chars().count(),
				..limits
			},
		),
		Err(EditError::InvalidName)
	));
	assert!(matches!(
		edit_icon_positions(
			&mut store,
			&[BAM_EDIT],
			ReadLimits {
				records: 0,
				..limits
			}
		),
		Err(EditError::TooManyEdits)
	));
	assert_eq!(store, NATIVE_STORE);
}

#[test]
#[ignore = "requires Python ds_store 1.3.4 and mac_alias 2.2.3"]
fn independent_reader_sees_changed_centers_and_preserved_opaque_value() {
	let mut native = NATIVE_STORE.to_vec();
	edit_icon_positions(&mut native, &[BAM_EDIT, BAR_EDIT], ReadLimits::default()).unwrap();
	run_oracle(&native, "native");
	let mut synthetic = store_with_opaque_record();
	edit_icon_positions(&mut synthetic, &[BAM_EDIT], ReadLimits::default()).unwrap();
	run_oracle(&synthetic, "synthetic");
}

fn replace_coordinates(expected: &mut [u8], old: (u32, u32), replacement: IconPositionEdit<'_>) {
	let old = coordinates(old.0, old.1);
	let offset = unique_offset(expected, &old);
	expected[offset..offset + old.len()]
		.copy_from_slice(&coordinates(replacement.x, replacement.y));
}

fn coordinates(x: u32, y: u32) -> [u8; 2 * size_of::<u32>()] {
	let mut encoded = [0; 2 * size_of::<u32>()];
	let (left, top) = encoded.split_at_mut(size_of::<u32>());
	left.copy_from_slice(&x.to_be_bytes());
	top.copy_from_slice(&y.to_be_bytes());
	encoded
}

fn unique_offset(bytes: &[u8], value: &[u8]) -> usize {
	let mut matches = bytes
		.windows(value.len())
		.enumerate()
		.filter_map(|(offset, candidate)| (candidate == value).then_some(offset));
	let offset = matches.next().expect("fixture is missing the wire bytes");
	assert!(matches.next().is_none(), "fixture wire bytes are ambiguous");
	offset
}

fn store_with_opaque_record() -> Vec<u8> {
	ds_parser::write(&[
		Record::new(BAM_EDIT.name, FieldKey::Iloc, IconLocation::new(104, 116)),
		Record::new(
			OPAQUE_NAME,
			FieldKey::Unknown(FourCC::from(OPAQUE_FIELD)),
			OPAQUE_VALUE.to_vec(),
		),
	])
	.unwrap()
}

fn run_oracle(store: &[u8], profile: &str) {
	let mut oracle = Command::new("python3")
		.args(["-B", "-c", ORACLE, profile])
		.stdin(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.unwrap();
	oracle.stdin.take().unwrap().write_all(store).unwrap();
	let output = oracle.wait_with_output().unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
}
