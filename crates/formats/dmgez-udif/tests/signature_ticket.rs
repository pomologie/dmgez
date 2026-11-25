use dmgez_udif::{
	Error, TRAILER_BYTES, has_stapled_ticket, signed_signature_range, validate_staple_layout,
	validate_ticket_addition,
};

const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const SIGNATURE_OFFSET_FIELD: usize = 296;
const SIGNATURE_LENGTH_FIELD: usize = 304;
const SIGNATURE_FIELDS_END: usize = 312;
const DATA_FORK_FIELDS: (usize, usize) = (24, 32);
const RESOURCE_FORK_FIELDS: (usize, usize) = (40, 48);
const XML_FIELDS: (usize, usize) = (216, 224);
const EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const BLOB_WRAPPER: u32 = 0xfade_0b01;
const CODE_DIRECTORY: u32 = 0xfade_0c02;
const DIRECTORY_SLOT: u32 = 0;
const CMS_SLOT: u32 = 0x0001_0000;
const TICKET_SLOT: u32 = 0x0001_0002;
const SUPERBLOB_HEADER: usize = 3 * size_of::<u32>();
const INDEX_ENTRY: usize = 2 * size_of::<u32>();

#[test]
fn allows_growth_of_only_the_trailer_signature_length() {
	let body_length = (UDZO.len() - TRAILER_BYTES) as u64;
	let (before, before_length) = trailer(body_length, 346);
	let (after, after_length) = trailer(body_length, 512);

	assert_eq!(
		signed_signature_range(&before, before_length).unwrap(),
		body_length..body_length + 346,
	);
	validate_staple_layout(&before, before_length, &after, after_length).unwrap();
	for field in (0..SIGNATURE_LENGTH_FIELD).chain(SIGNATURE_FIELDS_END..TRAILER_BYTES) {
		let mut changed = after;
		changed[field] ^= 1;
		assert!(validate_staple_layout(&before, before_length, &changed, after_length).is_err());
	}
	assert!(validate_staple_layout(&after, after_length, &before, before_length).is_err());
	assert!(validate_staple_layout(&before, before_length, &before, before_length).is_err());
	assert!(validate_staple_layout(&before, before_length, &after, after_length + 1).is_err());
}

#[test]
fn rejects_unchanged_trailer_ranges_backed_by_mutable_signature_bytes() {
	let body_length = (UDZO.len() - TRAILER_BYTES) as u64;
	for (offset_field, length_field) in [DATA_FORK_FIELDS, RESOURCE_FORK_FIELDS, XML_FIELDS] {
		for offset in [body_length, u64::MAX] {
			let (mut before, before_length) = trailer(body_length, 346);
			let (mut after, after_length) = trailer(body_length, 512);
			for trailer in [&mut before, &mut after] {
				trailer[offset_field..offset_field + size_of::<u64>()]
					.copy_from_slice(&offset.to_be_bytes());
				trailer[length_field..length_field + size_of::<u64>()]
					.copy_from_slice(&1u64.to_be_bytes());
			}
			assert!(signed_signature_range(&before, before_length).is_err());
			assert!(validate_staple_layout(&before, before_length, &after, after_length).is_err());
		}
	}
}

#[test]
fn admits_one_ticket_with_reordered_original_components() {
	let directory = component(CODE_DIRECTORY, b"synthetic code directory");
	let cms = component(BLOB_WRAPPER, b"synthetic signing payload");
	let ticket = component(BLOB_WRAPPER, b"synthetic framing test");
	let source = signature(&[(DIRECTORY_SLOT, &directory), (CMS_SLOT, &cms)]);
	let stapled = signature(&[
		(TICKET_SLOT, &ticket),
		(CMS_SLOT, &cms),
		(DIRECTORY_SLOT, &directory),
	]);

	assert!(!has_stapled_ticket(&source).unwrap());
	assert!(has_stapled_ticket(&stapled).unwrap());
	validate_ticket_addition(&source, &stapled).unwrap();
	assert!(validate_ticket_addition(&source, &source).is_err());
	assert!(matches!(
		validate_ticket_addition(&stapled, &stapled),
		Err(Error::Unsupported(_)),
	));
}

#[test]
fn rejects_changed_missing_or_added_non_ticket_components() {
	let directory = component(CODE_DIRECTORY, b"synthetic code directory");
	let cms = component(BLOB_WRAPPER, b"synthetic signing payload");
	let ticket = component(BLOB_WRAPPER, b"synthetic framing test");
	let changed = component(BLOB_WRAPPER, b"different signing payload");
	let source = signature(&[(DIRECTORY_SLOT, &directory), (CMS_SLOT, &cms)]);
	for staged in [
		signature(&[
			(DIRECTORY_SLOT, &directory),
			(CMS_SLOT, &changed),
			(TICKET_SLOT, &ticket),
		]),
		signature(&[(DIRECTORY_SLOT, &directory), (TICKET_SLOT, &ticket)]),
		signature(&[
			(DIRECTORY_SLOT, &directory),
			(CMS_SLOT, &cms),
			(TICKET_SLOT, &ticket),
			(CMS_SLOT + 5, &changed),
		]),
	] {
		assert!(validate_ticket_addition(&source, &staged).is_err());
	}
}

#[test]
fn rejects_duplicate_invalid_and_unindexed_ticket_data() {
	let directory = component(CODE_DIRECTORY, b"synthetic code directory");
	let ticket = component(BLOB_WRAPPER, b"synthetic framing test");
	let empty = component(BLOB_WRAPPER, b"");
	let valid = signature(&[(DIRECTORY_SLOT, &directory), (TICKET_SLOT, &ticket)]);
	let bad_wrapper = signature(&[(DIRECTORY_SLOT, &directory), (TICKET_SLOT, &directory)]);
	let empty_wrapper = signature(&[(DIRECTORY_SLOT, &directory), (TICKET_SLOT, &empty)]);
	let duplicate = signature(&[(TICKET_SLOT, &ticket), (TICKET_SLOT, &ticket)]);
	let mut overlapping = valid.clone();
	let first_offset = SUPERBLOB_HEADER + size_of::<u32>();
	let second_offset = first_offset + INDEX_ENTRY;
	let first = overlapping[first_offset..first_offset + size_of::<u32>()].to_vec();
	overlapping[second_offset..second_offset + size_of::<u32>()].copy_from_slice(&first);
	let mut unindexed = valid.clone();
	unindexed.push(0);
	let length = u32::try_from(unindexed.len()).unwrap();
	unindexed[4..8].copy_from_slice(&length.to_be_bytes());
	for malformed in [
		bad_wrapper,
		empty_wrapper,
		duplicate,
		overlapping,
		unindexed,
	] {
		assert!(has_stapled_ticket(&malformed).is_err());
	}
	for length in 0..valid.len() {
		assert!(has_stapled_ticket(&valid[..length]).is_err());
	}
	let mut over_budget = valid;
	over_budget[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(matches!(
		has_stapled_ticket(&over_budget),
		Err(Error::ResourceLimit(_)),
	));
}

fn trailer(offset: u64, length: u64) -> ([u8; TRAILER_BYTES], u64) {
	let mut trailer = *UDZO.last_chunk::<TRAILER_BYTES>().unwrap();
	trailer[SIGNATURE_OFFSET_FIELD..SIGNATURE_LENGTH_FIELD].copy_from_slice(&offset.to_be_bytes());
	trailer[SIGNATURE_LENGTH_FIELD..SIGNATURE_FIELDS_END].copy_from_slice(&length.to_be_bytes());
	(trailer, offset + length + TRAILER_BYTES as u64)
}

fn component(magic: u32, payload: &[u8]) -> Vec<u8> {
	let length = u32::try_from(2 * size_of::<u32>() + payload.len()).unwrap();
	[
		magic.to_be_bytes().as_slice(),
		&length.to_be_bytes(),
		payload,
	]
	.concat()
}

fn signature(slots: &[(u32, &[u8])]) -> Vec<u8> {
	let mut offset = SUPERBLOB_HEADER + slots.len() * INDEX_ENTRY;
	let length = offset + slots.iter().map(|(_, blob)| blob.len()).sum::<usize>();
	let mut signature = Vec::with_capacity(length);
	signature.extend(EMBEDDED_SIGNATURE.to_be_bytes());
	signature.extend(u32::try_from(length).unwrap().to_be_bytes());
	signature.extend(u32::try_from(slots.len()).unwrap().to_be_bytes());
	for (slot, blob) in slots {
		signature.extend(slot.to_be_bytes());
		signature.extend(u32::try_from(offset).unwrap().to_be_bytes());
		offset += blob.len();
	}
	for (_, blob) in slots {
		signature.extend_from_slice(blob);
	}
	signature
}
