use dmgez_udif::{Error, TRAILER_BYTES, check_trailer_header};

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const MAGIC_OFFSET: usize = 0;
const VERSION_OFFSET: usize = 4;
const HEADER_LENGTH_OFFSET: usize = 8;

#[test]
fn identifies_a_native_udif_trailer_without_the_reader() {
	let footer = UDRO.last_chunk::<TRAILER_BYTES>().unwrap();
	check_trailer_header(footer).unwrap();
}

#[test]
fn rejects_each_invalid_trailer_identity_field() {
	let valid = UDRO.last_chunk::<TRAILER_BYTES>().unwrap();
	let damaged = |offset: usize| {
		let mut footer = *valid;
		footer[offset] ^= 1;
		footer
	};
	assert!(matches!(
		check_trailer_header(&damaged(MAGIC_OFFSET)),
		Err(Error::InvalidFormat("missing koly trailer"))
	));
	assert!(matches!(
		check_trailer_header(&damaged(VERSION_OFFSET)),
		Err(Error::Unsupported("koly version"))
	));
	assert!(matches!(
		check_trailer_header(&damaged(HEADER_LENGTH_OFFSET)),
		Err(Error::Corrupt("koly header length"))
	));
}
