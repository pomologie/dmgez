//! Sampling and publication evidence for the D-OPS-02 optimization slice.

#![cfg(feature = "convert")]

use std::{fs, path::Path};

use dmgez_operations::{
	convert::{self, OutputCheck},
	optimize::{self, Error, Evaluation, Options, SampleLimits, Window},
};
use dmgez_udif::{
	SECTOR_BYTES,
	reader::{Image, Limits},
	writer::{Profile, WriteOptions, write_image},
};

const SECTOR: usize = SECTOR_BYTES as usize;
const CHUNK: usize = 8 * SECTOR;
const TRIAL_CHUNKS: usize = 7;
const TAIL_BYTES: usize = SECTOR;
const SOURCE: &str = "source.dmg";
const OUTPUT: &str = "optimized.dmg";

fn options() -> Options {
	Options {
		conversion: convert::Options {
			output: WriteOptions {
				chunk_bytes: CHUNK,
				..WriteOptions::default()
			},
			..convert::Options::default()
		},
		sample_limits: SampleLimits {
			max_chunks: 4,
			max_decoded_bytes: 4 * CHUNK as u64,
		},
	}
}

fn wrap(path: &Path, bytes: &[u8]) {
	let output = fs::File::create(path).unwrap();
	write_image(
		bytes,
		bytes.len() as u64,
		output,
		&WriteOptions {
			profile: Profile::Udro,
			..WriteOptions::default()
		},
	)
	.unwrap();
}

fn decode(path: &Path) -> Vec<u8> {
	let data = fs::read(path).unwrap();
	let mut image = Image::open(data.as_slice(), Limits::default()).unwrap();
	let mut raw = Vec::new();
	image.write_raw_to(&mut raw).unwrap();
	raw
}

fn wrapped_bytes(evaluation: &Evaluation) -> u64 {
	let Evaluation::Measured { wrapped_bytes, .. } = evaluation else {
		panic!("expected a measured candidate");
	};
	*wrapped_bytes
}

#[test]
fn evenly_samples_output_chunks_and_preserves_exact_bytes() {
	let tmp = tempfile::tempdir().unwrap();
	let source = tmp.path().join(SOURCE);
	let output = tmp.path().join(OUTPUT);
	let raw = vec![b'Q'; TRIAL_CHUNKS * CHUNK + TAIL_BYTES];
	wrap(&source, &raw);
	let report = optimize::file(&source, &output, &options()).unwrap();
	assert_eq!(
		report.windows,
		[
			Window {
				offset: 0,
				bytes: CHUNK
			},
			Window {
				offset: (2 * CHUNK) as u64,
				bytes: CHUNK
			},
			Window {
				offset: (4 * CHUNK) as u64,
				bytes: CHUNK
			},
			Window {
				offset: (7 * CHUNK) as u64,
				bytes: TAIL_BYTES
			},
		]
	);
	assert_eq!(
		report.sampled_decoded_bytes,
		(3 * CHUNK + TAIL_BYTES) as u64
	);
	assert_eq!(report.conversion.output_profile, Profile::Udzo);
	assert_eq!(
		report.conversion.output_check,
		OutputCheck::VerifiedAndCompared
	);
	assert_eq!(report.candidates[0].profile, Profile::Udro);
	assert_eq!(
		report.candidates[1].zlib_level,
		Some(options().conversion.output.zlib_level)
	);
	assert!(
		wrapped_bytes(&report.candidates[1].evaluation)
			< wrapped_bytes(&report.candidates[0].evaluation)
	);
	assert_eq!(decode(&output), raw);
	let other = tmp.path().join("repeat.dmg");
	let repeat = optimize::file(&source, &other, &options()).unwrap();
	assert_eq!(repeat.windows, report.windows);
	assert_eq!(
		repeat.conversion.output_profile,
		report.conversion.output_profile
	);
	assert_eq!(fs::read(other).unwrap(), fs::read(output).unwrap());
}

#[test]
fn incompressible_samples_break_equal_cost_toward_udro() {
	let tmp = tempfile::tempdir().unwrap();
	let source = tmp.path().join(SOURCE);
	let output = tmp.path().join(OUTPUT);
	let mut random = Vec::new();
	let mut state = 0x84d1_b6e3_c199_a438u64;
	for _ in 0..CHUNK / size_of::<u64>() {
		state ^= state << 13;
		state ^= state >> 7;
		state ^= state << 17;
		random.extend_from_slice(&state.to_le_bytes());
	}
	wrap(&source, &random);
	let report = optimize::file(&source, &output, &options()).unwrap();
	assert_eq!(report.conversion.output_profile, Profile::Udro);
	assert_eq!(
		wrapped_bytes(&report.candidates[0].evaluation),
		wrapped_bytes(&report.candidates[1].evaluation)
	);
	assert_eq!(decode(&output), random);
}

#[test]
fn refuses_exhausted_sample_budgets_and_reports_unavailable_candidates() {
	let tmp = tempfile::tempdir().unwrap();
	let source = tmp.path().join(SOURCE);
	let output = tmp.path().join(OUTPUT);
	let mut options = options();
	options.sample_limits.max_decoded_bytes = 0;
	assert!(matches!(
		optimize::file(&source, &output, &options),
		Err(Error::SampleBudget)
	));
	assert!(!output.exists());
	wrap(&source, &vec![b'X'; 2 * CHUNK]);
	options.sample_limits.max_decoded_bytes = CHUNK as u64 - 1;
	assert!(matches!(
		optimize::file(&source, &output, &options),
		Err(Error::SampleBudget)
	));
	assert!(!output.exists());
	options.sample_limits.max_decoded_bytes = CHUNK as u64;
	options.conversion.output.max_buffer_bytes = CHUNK;
	assert!(matches!(
		optimize::file(&source, &output, &options),
		Err(Error::Candidates { .. })
	));
	assert!(!output.exists());
	options.conversion.output.max_buffer_bytes = 2 * CHUNK;
	let report = optimize::file(&source, &output, &options).unwrap();
	assert_eq!(
		report.windows,
		[Window {
			offset: CHUNK as u64,
			bytes: CHUNK
		}]
	);
	assert_eq!(report.conversion.output_profile, Profile::Udro);
	assert!(matches!(
		report.candidates[1].evaluation,
		Evaluation::Unavailable(_)
	));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires Apple hdiutil"]
fn apple_exports_the_selected_profile_byte_for_byte() {
	use std::process::Command;
	const HDIUTIL: &str = "/usr/bin/hdiutil";
	let tmp = tempfile::tempdir().unwrap();
	let source = tmp.path().join(SOURCE);
	let output = tmp.path().join(OUTPUT);
	let exported = tmp.path().join("export.cdr");
	let raw = vec![b'Q'; TRIAL_CHUNKS * CHUNK + TAIL_BYTES];
	wrap(&source, &raw);
	let report = optimize::file(&source, &output, &options()).unwrap();
	assert_eq!(report.conversion.output_profile, Profile::Udzo);
	assert!(
		Command::new(HDIUTIL)
			.arg("verify")
			.arg(&output)
			.status()
			.unwrap()
			.success()
	);
	assert!(
		Command::new(HDIUTIL)
			.arg("convert")
			.arg(&output)
			.args(["-format", "UDTO", "-o"])
			.arg(&exported)
			.status()
			.unwrap()
			.success()
	);
	assert_eq!(fs::read(exported).unwrap(), raw);
}
