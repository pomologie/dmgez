use dmgez_udif::{Error, embedded_code_directory};

const SIGNATURE_MAGIC: u32 = 0xfade_0cc0;
const DIRECTORY_MAGIC: u32 = 0xfade_0c02;
const REQUIREMENTS_MAGIC: u32 = 0xfade_0c01;
const WRAPPER_MAGIC: u32 = 0xfade_0b01;
const DIRECTORY_SLOT: u32 = 0;
const REQUIREMENTS_SLOT: u32 = 2;
const ALTERNATE_SLOT: u32 = 0x1000;
const CMS_SLOT: u32 = 0x1_0000;
const TICKET_SLOT: u32 = 0x1_0002;
const VERSION_FIELD: usize = 8;
const HASH_OFFSET_FIELD: usize = 16;
const IDENTIFIER_FIELD: usize = 20;
const SPECIAL_COUNT_FIELD: usize = 24;
const CODE_COUNT_FIELD: usize = 28;
const LIMIT_FIELD: usize = 32;
const HASH_SIZE_FIELD: usize = 36;
const HASH_TYPE_FIELD: usize = 37;
const PAGE_FIELD: usize = 39;
const SCATTER_FIELD: usize = 44;
const HEADER_BYTES: usize = 48;
const IDENTIFIER: &[u8] = b"org.dmgez.fixture\0";
const HASH_BYTES: usize = 32;
const SPECIAL_COUNT: usize = 6;
const SHA256_TYPE: u8 = 2;
const VERSION: u32 = 0x2_0100;
const LIMIT: u32 = 3_084;
const BODY_HASH: [u8; HASH_BYTES] = [11; HASH_BYTES];
const TRAILER_HASH: [u8; HASH_BYTES] = [22; HASH_BYTES];
const REQUIREMENTS_HASH: [u8; HASH_BYTES] = [33; HASH_BYTES];

#[test]
fn borrows_exact_digest_inputs_for_the_native_geometry() {
	let directory = code_directory();
	let requirements = requirements_set();
	let cms = wrap(WRAPPER_MAGIC, b"synthetic");
	let ticket = wrap(WRAPPER_MAGIC, b"framing only");
	let signature = envelope(&[
		(TICKET_SLOT, &ticket),
		(REQUIREMENTS_SLOT, &requirements),
		(DIRECTORY_SLOT, &directory),
		(CMS_SLOT, &cms),
	]);
	let result = embedded_code_directory(&signature).unwrap();
	assert_eq!(result.signing_limit, u64::from(LIMIT));
	assert_eq!(result.directory, directory);
	assert_eq!(result.requirements, requirements);
	assert_eq!(*result.body_hash, BODY_HASH);
	assert_eq!(*result.trailer_hash, TRAILER_HASH);
	assert_eq!(*result.requirements_hash, REQUIREMENTS_HASH);
}

#[test]
fn treats_other_geometries_and_alternate_directories_as_unsupported() {
	let directory = code_directory();
	let requirements = requirements_set();
	let alternate = envelope(&[
		(DIRECTORY_SLOT, &directory),
		(REQUIREMENTS_SLOT, &requirements),
		(ALTERNATE_SLOT, &directory),
	]);
	assert!(matches!(
		embedded_code_directory(&alternate),
		Err(Error::Unsupported(_)),
	));

	for (field, value) in [
		(VERSION_FIELD, 0x2_0400),
		(SPECIAL_COUNT_FIELD, 7),
		(CODE_COUNT_FIELD, 2),
		(SCATTER_FIELD, 48),
	] {
		let mut directory = directory.clone();
		put_word(&mut directory, field, value);
		assert!(matches!(
			embedded_code_directory(&with_requirements(&directory)),
			Err(Error::Unsupported(_)),
		));
	}
	for (field, value) in [
		(HASH_SIZE_FIELD, 20),
		(HASH_TYPE_FIELD, 1),
		(PAGE_FIELD, 12),
		(HEADER_BYTES + IDENTIFIER.len() + HASH_BYTES, 1),
	] {
		let mut directory = directory.clone();
		directory[field] = value;
		assert!(matches!(
			embedded_code_directory(&with_requirements(&directory)),
			Err(Error::Unsupported(_)),
		));
	}
}

#[test]
fn rejects_corrupt_ranges_in_an_admitted_geometry() {
	let directory = code_directory();
	for (field, value) in [
		(HASH_OFFSET_FIELD, 0),
		(HASH_OFFSET_FIELD, u32::MAX),
		(IDENTIFIER_FIELD, 0),
		(IDENTIFIER_FIELD, u32::MAX),
		(LIMIT_FIELD, 0),
	] {
		let mut directory = directory.clone();
		put_word(&mut directory, field, value);
		assert!(matches!(
			embedded_code_directory(&with_requirements(&directory)),
			Err(Error::Corrupt(_)),
		));
	}

	let mut unterminated = directory.clone();
	unterminated[HEADER_BYTES + IDENTIFIER.len() - 1] = b'x';
	assert!(matches!(
		embedded_code_directory(&with_requirements(&unterminated)),
		Err(Error::Corrupt(_)),
	));
	let wrong_magic = wrap(WRAPPER_MAGIC, &0u32.to_be_bytes());
	let short_set = wrap(REQUIREMENTS_MAGIC, &[]);
	for requirements in [wrong_magic, short_set] {
		let signature = envelope(&[
			(DIRECTORY_SLOT, &directory),
			(REQUIREMENTS_SLOT, &requirements),
		]);
		assert!(matches!(
			embedded_code_directory(&signature),
			Err(Error::Corrupt(_)),
		));
	}
}

fn code_directory() -> Vec<u8> {
	let mut directory = vec![0; HEADER_BYTES];
	put_word(&mut directory, 0, DIRECTORY_MAGIC);
	put_word(&mut directory, VERSION_FIELD, VERSION);
	let code_offset = HEADER_BYTES + IDENTIFIER.len() + SPECIAL_COUNT * HASH_BYTES;
	put_word(
		&mut directory,
		HASH_OFFSET_FIELD,
		u32::try_from(code_offset).unwrap(),
	);
	put_word(
		&mut directory,
		IDENTIFIER_FIELD,
		u32::try_from(HEADER_BYTES).unwrap(),
	);
	put_word(
		&mut directory,
		SPECIAL_COUNT_FIELD,
		u32::try_from(SPECIAL_COUNT).unwrap(),
	);
	put_word(&mut directory, CODE_COUNT_FIELD, 1);
	put_word(&mut directory, LIMIT_FIELD, LIMIT);
	directory[HASH_SIZE_FIELD] = u8::try_from(HASH_BYTES).unwrap();
	directory[HASH_TYPE_FIELD] = SHA256_TYPE;
	directory.extend(IDENTIFIER);
	directory.extend(TRAILER_HASH);
	directory.extend([0; 3 * HASH_BYTES]);
	directory.extend(REQUIREMENTS_HASH);
	directory.extend([0; HASH_BYTES]);
	directory.extend(BODY_HASH);
	let length = u32::try_from(directory.len()).unwrap();
	put_word(&mut directory, size_of::<u32>(), length);
	directory
}

fn put_word(bytes: &mut [u8], offset: usize, value: u32) {
	bytes[offset..offset + size_of::<u32>()].copy_from_slice(&value.to_be_bytes());
}

fn with_requirements(directory: &[u8]) -> Vec<u8> {
	envelope(&[
		(DIRECTORY_SLOT, directory),
		(REQUIREMENTS_SLOT, &requirements_set()),
	])
}

fn requirements_set() -> Vec<u8> {
	wrap(REQUIREMENTS_MAGIC, &0u32.to_be_bytes())
}

fn wrap(magic: u32, payload: &[u8]) -> Vec<u8> {
	let size = u32::try_from(2 * size_of::<u32>() + payload.len()).unwrap();
	[magic.to_be_bytes().as_slice(), &size.to_be_bytes(), payload].concat()
}

fn envelope(slots: &[(u32, &[u8])]) -> Vec<u8> {
	let mut offset = size_of::<u32>() * (3 + slots.len() * 2);
	let size = offset + slots.iter().map(|(_, blob)| blob.len()).sum::<usize>();
	let mut signature = Vec::with_capacity(size);
	signature.extend(SIGNATURE_MAGIC.to_be_bytes());
	signature.extend(u32::try_from(size).unwrap().to_be_bytes());
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
