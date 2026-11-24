//! ULFO wire evidence and codec feature guards for D-UDIF-05.

#![cfg(any(feature = "read", feature = "write"))]

#[cfg(all(feature = "write", any(feature = "read", not(feature = "lzfse"))))]
use dmgez_udif::writer::{Profile, WriteOptions, write_image};
#[cfg(feature = "read")]
use dmgez_udif::{
	Error,
	reader::{Image, Limits, RunEncoding},
};

#[cfg(feature = "read")]
const NATIVE: &[u8] = include_bytes!("fixtures/apple-ulfo.dmg");
#[cfg(all(feature = "read", feature = "lzfse"))]
const NATIVE_CHUNK: usize = 1 << 20;
#[cfg(all(feature = "read", feature = "lzfse"))]
const NATIVE_PATTERN: &[u8; 4] = b"ABCD";

#[cfg(all(feature = "read", feature = "lzfse"))]
#[test]
fn decodes_apple_lzfse_and_lzvn_runs_and_checks_all_checksum_domains() {
	use dmgez_udif::{SECTOR_BYTES, reader::CheckStatus};

	const TAIL: &[u8; 4] = b"ULFO";
	let mut image = Image::open(NATIVE, Limits::default()).unwrap();
	let runs: Vec<_> = image.runs().collect();
	assert_eq!(runs.len(), 3);
	assert!(
		runs.iter()
			.all(|run| run.encoding == RunEncoding::Lzfse && run.encoding.can_decode())
	);
	let mut crossing = [0; 8];
	image
		.read_exact_at((NATIVE_CHUNK - 4) as u64, &mut crossing)
		.unwrap();
	assert_eq!(&crossing[..4], NATIVE_PATTERN);
	assert_eq!(&crossing[4..], &[0; 4]);
	let mut decoded = Vec::new();
	image.write_raw_to(&mut decoded).unwrap();
	assert_eq!(decoded.len(), 2 * NATIVE_CHUNK + SECTOR_BYTES as usize);
	assert!(
		decoded[..NATIVE_CHUNK]
			.as_chunks::<{ NATIVE_PATTERN.len() }>()
			.0
			.iter()
			.all(|chunk| chunk == NATIVE_PATTERN)
	);
	assert!(
		decoded[NATIVE_CHUNK..2 * NATIVE_CHUNK]
			.iter()
			.all(|&byte| byte == 0)
	);
	assert!(
		decoded[2 * NATIVE_CHUNK..]
			.as_chunks::<{ TAIL.len() }>()
			.0
			.iter()
			.all(|chunk| chunk == TAIL)
	);
	let verified = image.verify().unwrap();
	assert_eq!(verified.data_fork, CheckStatus::Passed);
	assert_eq!(verified.tables, [CheckStatus::Passed]);
	assert_eq!(verified.master, CheckStatus::Passed);
}

#[cfg(all(feature = "read", feature = "lzfse"))]
#[test]
fn rejects_out_of_budget_runs_and_missing_end_of_stream() {
	const LAST_BYTE: usize = 1;
	let limits = Limits {
		max_decoded_bytes: NATIVE_CHUNK - 1,
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
	let run = image.runs().next().unwrap();
	let mut damaged = NATIVE.to_vec();
	damaged[run.physical.end as usize - LAST_BYTE] = 0;
	let mut damaged = Image::open(damaged.as_slice(), Limits::default()).unwrap();
	assert!(matches!(
		damaged.read_exact_at(0, &mut [0]),
		Err(Error::InvalidLzfse {
			offset: 0,
			logical_offset: 0
		})
	));
}

#[cfg(all(feature = "read", not(feature = "lzfse")))]
#[test]
fn recognizes_disabled_lzfse_without_decoding() {
	let mut image = Image::open(NATIVE, Limits::default()).unwrap();
	let encoding = image.runs().next().unwrap().encoding;
	assert_eq!(encoding, RunEncoding::Lzfse);
	assert!(!encoding.can_decode());
	assert!(matches!(
		image.read_exact_at(0, &mut [0]),
		Err(Error::FeatureNotCompiled("lzfse"))
	));
}

#[cfg(all(feature = "write", not(feature = "lzfse")))]
#[test]
fn rejects_disabled_ulfo_before_writing() {
	let options = WriteOptions {
		profile: Profile::Ulfo,
		..WriteOptions::default()
	};
	let mut output = Vec::new();
	assert!(matches!(
		write_image(&[][..], dmgez_udif::SECTOR_BYTES, &mut output, &options),
		Err(dmgez_udif::Error::FeatureNotCompiled("lzfse"))
	));
	assert!(output.is_empty());
}

#[cfg(all(feature = "read", feature = "write", feature = "lzfse"))]
#[test]
fn writes_lzfse_zero_and_raw_fallback_runs() {
	let mut raw = vec![b'A'; NATIVE_CHUNK];
	raw.resize(2 * NATIVE_CHUNK, 0);
	// A fixed xorshift generator makes the third chunk incompressible.
	let mut state = 0x5ce3_09a7_2710_d198u64;
	for _ in 0..NATIVE_CHUNK / size_of::<u64>() {
		state ^= state << 13;
		state ^= state >> 7;
		state ^= state << 17;
		raw.extend_from_slice(&state.to_le_bytes());
	}
	let options = WriteOptions {
		profile: Profile::Ulfo,
		..WriteOptions::default()
	};
	let mut image = Vec::new();
	let stats = write_image(raw.as_slice(), raw.len() as u64, &mut image, &options).unwrap();
	assert_eq!(
		(stats.compressed_runs, stats.zero_runs, stats.raw_runs),
		(1, 1, 1)
	);

	#[cfg(feature = "parallel")]
	{
		use dmgez_udif::writer::Execution;

		let parallel = WriteOptions {
			execution: Execution::Parallel { workers: 2 },
			..options.clone()
		};
		let mut other = Vec::new();
		let parallel_stats =
			write_image(raw.as_slice(), raw.len() as u64, &mut other, &parallel).unwrap();
		assert_eq!(image, other);
		assert_eq!(
			parallel_stats.accounted_payload_bytes,
			2 * stats.accounted_payload_bytes
		);
	}

	let rejected = WriteOptions {
		max_buffer_bytes: stats.accounted_payload_bytes - 1,
		..options
	};
	let mut empty = Vec::new();
	assert!(matches!(
		write_image(raw.as_slice(), raw.len() as u64, &mut empty, &rejected),
		Err(Error::ResourceLimit(_))
	));
	assert!(empty.is_empty());

	let mut image = Image::open(image.as_slice(), Limits::default()).unwrap();
	assert!(image.runs().map(|run| run.encoding).eq([
		RunEncoding::Lzfse,
		RunEncoding::Zero,
		RunEncoding::Raw
	]));
	image.verify().unwrap();
	let mut roundtrip = Vec::new();
	image.write_raw_to(&mut roundtrip).unwrap();
	assert_eq!(roundtrip, raw);
}
