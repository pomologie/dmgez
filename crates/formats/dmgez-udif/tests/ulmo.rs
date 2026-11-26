//! Apple ULMO interoperability and codec feature bounds.

#![cfg(any(feature = "read", feature = "write"))]

#[cfg(any(feature = "read", not(feature = "lzma")))]
use dmgez_udif::Error;
#[cfg(feature = "read")]
use dmgez_udif::reader::{Image, Limits, RunEncoding};
#[cfg(all(feature = "write", any(feature = "read", not(feature = "lzma"))))]
use dmgez_udif::writer::{Profile, WriteOptions, write_image};

#[cfg(feature = "read")]
const NATIVE: &[u8] = include_bytes!("fixtures/apple-ulmo.dmg");
#[cfg(all(feature = "read", feature = "lzma"))]
const CHUNK: usize = 1 << 20;

#[cfg(all(feature = "read", feature = "lzma"))]
#[test]
fn decodes_apple_ulmo_and_validates_exact_extent_and_checksums() {
	use dmgez_udif::{SECTOR_BYTES, reader::CheckStatus};

	const PATTERN: &[u8] = b"ABCD";
	const TAIL: &[u8] = b"ULFO";
	let mut image = Image::open(NATIVE, Limits::default()).unwrap();
	assert_eq!(image.runs().count(), 3);
	assert!(
		image
			.runs()
			.all(|run| run.encoding == RunEncoding::Lzma2 && run.encoding.can_decode())
	);
	let mut crossing = [0; 2 * PATTERN.len()];
	image
		.read_exact_at((CHUNK - PATTERN.len()) as u64, &mut crossing)
		.unwrap();
	assert_eq!(&crossing[..PATTERN.len()], PATTERN);
	assert!(crossing[PATTERN.len()..].iter().all(|&byte| byte == 0));

	let mut expected = PATTERN.repeat(CHUNK / PATTERN.len());
	expected.resize(2 * CHUNK, 0);
	expected.extend_from_slice(&TAIL.repeat(SECTOR_BYTES as usize / TAIL.len()));
	let mut actual = Vec::new();
	image.write_raw_to(&mut actual).unwrap();
	assert_eq!(actual, expected);
	let checks = image.verify().unwrap();
	assert_eq!(checks.data_fork, CheckStatus::Passed);
	assert_eq!(checks.tables, [CheckStatus::Passed]);
	assert_eq!(checks.master, CheckStatus::Passed);
}

#[cfg(all(feature = "read", feature = "lzma"))]
#[test]
fn rejects_budget_and_corrupt_native_footer() {
	let limits = Limits {
		max_decoded_bytes: CHUNK - 1,
		..Limits::default()
	};
	assert!(matches!(
		Image::open(NATIVE, limits),
		Err(Error::ResourceLimit(_))
	));
	let limits = Limits {
		max_compressed_bytes: 0,
		..Limits::default()
	};
	assert!(matches!(
		Image::open(NATIVE, limits),
		Err(Error::ResourceLimit(_))
	));
	let image = Image::open(NATIVE, Limits::default()).unwrap();
	let first = image.runs().next().unwrap();
	let mut damaged = NATIVE.to_vec();
	damaged[first.physical.end as usize - 1] ^= 1;
	let image = Image::open(damaged.as_slice(), Limits::default()).unwrap();
	assert!(matches!(
		image.read_exact_at(0, &mut [0]),
		Err(Error::InvalidLzma {
			offset: 0,
			logical_offset: 0
		})
	));
}

#[cfg(all(feature = "read", not(feature = "lzma")))]
#[test]
fn disabled_codec_still_exposes_the_native_run_type() {
	let image = Image::open(NATIVE, Limits::default()).unwrap();
	let first = image.runs().next().unwrap();
	assert_eq!(first.encoding, RunEncoding::Lzma2);
	assert!(!first.encoding.can_decode());
	assert!(matches!(
		image.read_exact_at(0, &mut [0]),
		Err(Error::FeatureNotCompiled("lzma"))
	));
}

#[cfg(all(feature = "write", not(feature = "lzma")))]
#[test]
fn disabled_profile_rejects_before_emitting_any_bytes() {
	let mut output = Vec::new();
	let options = WriteOptions {
		profile: Profile::Ulmo,
		..WriteOptions::default()
	};
	assert!(matches!(
		write_image(&[][..], dmgez_udif::SECTOR_BYTES, &mut output, &options),
		Err(Error::FeatureNotCompiled("lzma"))
	));
	assert!(output.is_empty());
}

#[cfg(all(feature = "read", feature = "write", feature = "lzma"))]
#[test]
fn emits_raw_and_zero_runs_alongside_lzma2() {
	let mut raw = vec![b'A'; CHUNK];
	raw.resize(2 * CHUNK, 0);
	let mut state = 0x5ce3_09a7_2710_d198u64;
	for _ in 0..CHUNK / size_of::<u64>() {
		state ^= state << 13;
		state ^= state >> 7;
		state ^= state << 17;
		raw.extend_from_slice(&state.to_le_bytes());
	}
	let options = WriteOptions {
		profile: Profile::Ulmo,
		..WriteOptions::default()
	};
	let mut stored = Vec::new();
	let report = write_image(raw.as_slice(), raw.len() as u64, &mut stored, &options).unwrap();
	assert_eq!(
		(report.compressed_runs, report.zero_runs, report.raw_runs),
		(1, 1, 1)
	);
	let mut image = Image::open(stored.as_slice(), Limits::default()).unwrap();
	assert!(image.runs().map(|run| run.encoding).eq([
		RunEncoding::Lzma2,
		RunEncoding::Zero,
		RunEncoding::Raw
	]));
	image.verify().unwrap();
	let mut restored = Vec::new();
	image.write_raw_to(&mut restored).unwrap();
	assert_eq!(restored, raw);

	let tight = WriteOptions {
		max_buffer_bytes: report.accounted_payload_bytes - 1,
		..options
	};
	let mut output = Vec::new();
	assert!(matches!(
		write_image(raw.as_slice(), raw.len() as u64, &mut output, &tight),
		Err(Error::ResourceLimit(_))
	));
	assert!(output.is_empty());
}
