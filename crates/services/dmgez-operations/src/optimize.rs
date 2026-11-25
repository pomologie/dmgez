//! Deterministic profile selection from bounded logical samples.

use std::{
	collections::TryReserveError,
	io,
	num::NonZeroUsize,
	path::Path,
	time::{Duration, Instant},
};

use dmgez_udif::{
	Error as UdifError,
	reader::ReadAt,
	writer::{Execution, Profile, WriteOptions, write_image},
};
use thiserror::Error;

use crate::convert;

const MIB: u64 = 1 << 20;
const DEFAULT_SAMPLE_CHUNKS: u16 = 8;
const MAX_SAMPLE_CHUNKS: u16 = 64;
const DEFAULT_SAMPLE_BYTES: u64 = DEFAULT_SAMPLE_CHUNKS as u64 * MIB;

/// Limits for representative samples. Each sample is one whole output-aligned
/// chunk, including a short final chunk if selected.
#[derive(Debug, Clone, Copy)]
pub struct SampleLimits {
	/// Maximum number of sampled chunks. The allowed range is 1 through 64.
	pub max_chunks: u16,
	/// Maximum logical bytes supplied to each candidate across the selected
	/// windows. For compressed sources the reader may inflate complete source
	/// runs under the conversion input limits.
	pub max_decoded_bytes: u64,
}

impl Default for SampleLimits {
	fn default() -> Self {
		Self {
			max_chunks: DEFAULT_SAMPLE_CHUNKS,
			max_decoded_bytes: DEFAULT_SAMPLE_BYTES,
		}
	}
}

/// Source admission, output limits and sampling budget. The conversion writer
/// profile is selected by this operation. Its zlib level applies to UDZO.
#[derive(Debug, Clone, Default)]
pub struct Options {
	pub conversion: convert::Options,
	pub sample_limits: SampleLimits,
}

/// A sampled extent within the source's logical disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
	pub offset: u64,
	pub bytes: usize,
}

/// Measured or unavailable compression candidate.
#[derive(Debug)]
#[non_exhaustive]
pub enum Evaluation {
	/// Sum of complete, independently wrapped sample images. This includes a
	/// fresh table, resources and trailer per window. Duration covers serial
	/// sample encoding to a discard sink and excludes source reads.
	Measured {
		wrapped_bytes: u64,
		elapsed: Duration,
	},
	/// The candidate was rejected by writer admission for the sample or output.
	Unavailable(UdifError),
}

#[derive(Debug)]
#[non_exhaustive]
pub struct Candidate {
	pub profile: Profile,
	pub zlib_level: Option<u8>,
	pub evaluation: Evaluation,
}

/// Sample evidence and the actual verified conversion result. Measurements
/// describe the chosen windows, with no prediction of complete image size.
#[derive(Debug)]
#[non_exhaustive]
pub struct Report {
	pub windows: Vec<Window>,
	/// Total logical bytes in the selected windows.
	pub sampled_decoded_bytes: u64,
	pub candidates: [Candidate; 2],
	pub conversion: convert::Report,
}

/// Optimization did not result in a verified published file.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Conversion(#[from] convert::FileError),
	#[error("sample chunk count must be between 1 and {MAX_SAMPLE_CHUNKS}")]
	SampleCount,
	#[error("sample byte budget cannot hold one aligned chunk")]
	SampleBudget,
	#[error("could not reserve a sample buffer")]
	Memory(#[from] TryReserveError),
	#[error("no compression candidate fits the sample and output bounds")]
	Candidates { candidates: [Candidate; 2] },
	#[error("could not read a logical sample at byte {offset}")]
	Read {
		offset: u64,
		#[source]
		error: convert::Error,
	},
	#[error("could not measure the {profile:?} profile")]
	Measure {
		profile: Profile,
		#[source]
		error: UdifError,
	},
}

/// Sample qualified UDRO and UDZO output for a verified source. Choose the
/// smaller measured byte count with UDRO winning ties. Timings are reported
/// independently of selection. Publish the chosen profile using the same
/// opened source and staged byte-for-byte comparison as file conversion.
///
/// # Errors
/// Refuses unqualified or changed inputs, invalid budgets, failed sampling,
/// and failed encoding or publication. The destination must be new.
pub fn file(source: &Path, destination: &Path, options: &Options) -> Result<Report, Error> {
	validate_limits(options.sample_limits)?;
	let mut conversion = options.conversion.clone();
	conversion.output.profile = Profile::Udro;
	let source = convert::open_source(source, destination)?;
	let (mut plan, checks) =
		convert::prepare(source, &conversion).map_err(convert::FileError::from)?;
	let selection = select(&mut plan, &conversion.output, options.sample_limits)?;
	conversion.output.profile = selection.profile;
	let conversion = convert::publish_plan(plan, checks, destination, &conversion)?;
	Ok(Report {
		windows: selection.windows,
		sampled_decoded_bytes: selection.sampled_decoded_bytes,
		candidates: selection.candidates,
		conversion,
	})
}

fn validate_limits(limits: SampleLimits) -> Result<(), Error> {
	if !(1..=MAX_SAMPLE_CHUNKS).contains(&limits.max_chunks) {
		return Err(Error::SampleCount);
	}
	Ok(())
}

struct Selection {
	profile: Profile,
	windows: Vec<Window>,
	sampled_decoded_bytes: u64,
	candidates: [Candidate; 2],
}

fn select<S: ReadAt>(
	plan: &mut convert::Plan<S>,
	output: &WriteOptions,
	limits: SampleLimits,
) -> Result<Selection, Error> {
	let windows = windows(plan.logical_bytes(), output.chunk_bytes, limits)?;
	let capacity = windows
		.iter()
		.map(|window| window.bytes)
		.max()
		.ok_or(Error::SampleBudget)?;
	let mut candidates =
		[Profile::Udro, Profile::Udzo].map(|profile| trial(output, profile, capacity));
	if candidates
		.iter()
		.all(|candidate| matches!(candidate.evaluation, Evaluation::Unavailable(_)))
	{
		return Err(Error::Candidates { candidates });
	}
	let mut buffer = Vec::new();
	buffer.try_reserve_exact(capacity)?;
	buffer.resize(capacity, 0);
	for window in &windows {
		let sample = &mut buffer[..window.bytes];
		plan.read_exact_at(window.offset, sample)
			.map_err(|error| Error::Read {
				offset: window.offset,
				error,
			})?;
		for candidate in &mut candidates {
			measure(candidate, output, capacity, sample)?;
		}
	}
	let winner = candidates
		.iter()
		.filter_map(|candidate| match candidate.evaluation {
			Evaluation::Measured { wrapped_bytes, .. } => Some((wrapped_bytes, candidate.profile)),
			Evaluation::Unavailable(_) => None,
		})
		.min_by_key(|(bytes, profile)| (*bytes, *profile != Profile::Udro))
		.map(|(_, profile)| profile);
	let Some(profile) = winner else {
		return Err(Error::Candidates { candidates });
	};
	Ok(Selection {
		profile,
		sampled_decoded_bytes: windows.iter().map(|window| window.bytes as u64).sum(),
		windows,
		candidates,
	})
}

fn windows(logical: u64, chunk_bytes: usize, limits: SampleLimits) -> Result<Vec<Window>, Error> {
	let chunk = chunk_bytes as u64;
	let available = logical.div_ceil(chunk);
	let count = available
		.min(u64::from(limits.max_chunks))
		.min(limits.max_decoded_bytes / logical.min(chunk));
	if count == 0 {
		return Err(Error::SampleBudget);
	}
	Ok((0..count)
		.map(|ordinal| {
			let index = if count == 1 {
				available / 2
			} else {
				ordinal * (available - 1) / (count - 1)
			};
			let offset = index * chunk;
			Window {
				offset,
				bytes: (logical - offset).min(chunk) as usize,
			}
		})
		.collect())
}

fn trial(output: &WriteOptions, profile: Profile, capacity: usize) -> Candidate {
	let output = WriteOptions {
		profile,
		..output.clone()
	};
	let probe = probe_options(&output, profile, capacity);
	let available = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
	let admission = output
		.admit(available)
		.and_then(|_| probe.admit(NonZeroUsize::MIN));
	let evaluation = match admission {
		Ok(_) => Evaluation::Measured {
			wrapped_bytes: 0,
			elapsed: Duration::ZERO,
		},
		Err(error) => Evaluation::Unavailable(error),
	};
	Candidate {
		profile,
		zlib_level: (profile == Profile::Udzo).then_some(output.zlib_level),
		evaluation,
	}
}

fn probe_options(output: &WriteOptions, profile: Profile, capacity: usize) -> WriteOptions {
	WriteOptions {
		profile,
		execution: Execution::Serial,
		max_physical_bytes: u64::MAX,
		max_buffer_bytes: output.max_buffer_bytes.saturating_sub(capacity),
		..output.clone()
	}
}

fn measure(
	candidate: &mut Candidate,
	output: &WriteOptions,
	capacity: usize,
	sample: &[u8],
) -> Result<(), Error> {
	if let Evaluation::Measured {
		wrapped_bytes,
		elapsed,
	} = &mut candidate.evaluation
	{
		let options = probe_options(output, candidate.profile, capacity);
		let start = Instant::now();
		let report =
			write_image(sample, sample.len() as u64, io::sink(), &options).map_err(|error| {
				Error::Measure {
					profile: candidate.profile,
					error,
				}
			})?;
		*elapsed += start.elapsed();
		*wrapped_bytes += report.physical_bytes;
	}
	Ok(())
}

#[cfg(all(test, unix))]
mod tests {
	use dmgez_udif::{
		SECTOR_BYTES,
		reader::{Image, Limits},
	};
	use std::{fs, os::unix::fs::symlink};

	use super::*;

	#[test]
	fn publishing_uses_the_sampled_handle_even_if_the_source_path_is_retargeted() {
		let directory = tempfile::tempdir().unwrap();
		let sampled = directory.path().join("sampled.dmg");
		let replacement = directory.path().join("replacement.dmg");
		let source = directory.path().join("current-source");
		let destination = directory.path().join("published.dmg");
		let mut options = convert::Options::default();
		options.output.profile = Profile::Udro;
		let encode = |path: &Path, byte: u8| {
			let data = vec![byte; SECTOR_BYTES as usize];
			write_image(
				data.as_slice(),
				SECTOR_BYTES,
				fs::File::create(path).unwrap(),
				&options.output,
			)
			.unwrap();
		};
		encode(&sampled, b'S');
		encode(&replacement, b'R');
		symlink(&sampled, &source).unwrap();
		let handle = convert::open_source(&source, &destination).unwrap();
		let (mut plan, checks) = convert::prepare(handle, &options).unwrap();
		let selection = select(&mut plan, &options.output, SampleLimits::default()).unwrap();
		fs::remove_file(&source).unwrap();
		symlink(&replacement, &source).unwrap();
		options.output.profile = selection.profile;
		convert::publish_plan(plan, checks, &destination, &options).unwrap();

		let stored = fs::read(destination).unwrap();
		let mut decoded = Image::open(stored.as_slice(), Limits::default()).unwrap();
		let mut bytes = Vec::new();
		decoded.write_raw_to(&mut bytes).unwrap();
		assert_eq!(bytes, vec![b'S'; SECTOR_BYTES as usize]);
	}
}
