//! Same-geometry conversion of admitted UDRO and UDZO profiles.

use std::{
	cell::RefCell,
	fs::{self, File},
	io::{self, Read, Seek, SeekFrom, Write},
	num::NonZeroUsize,
	path::Path,
};

use dmgez_core::output::{PublishError, StagedReader, publish_new};
use dmgez_udif::{
	Error as UdifError,
	reader::{FileSource, Image, Limits, ReadAt, Verification},
	writer::{
		Cancellation, Profile, RebuildMetadata, WriteOptions, WriteReport, write_rebuilt_image,
	},
};
use thiserror::Error;

const COMPARE_WINDOW_BYTES: usize = 64 << 10;

/// Bounds for source admission and output encoding. The output profile must
/// be UDRO or UDZO. Both presets preserve the input's virtual geometry.
#[derive(Debug, Clone, Default)]
pub struct Options {
	pub input: Limits,
	pub output: WriteOptions,
}

/// Permission to discard an embedded signature during file conversion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignaturePolicy {
	/// Require an unsigned container.
	#[default]
	RequireUnsigned,
	/// Allow removal of the original signature and any associated ticket.
	Discard,
}

/// Output validation performed before a successful return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputCheck {
	/// The streaming writer completed all of its structural checks.
	Writer,
	/// The staged image was reopened, verified and compared byte-for-byte.
	VerifiedAndCompared,
}

/// Trust state of the admitted source and generated image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrustState {
	/// Both images use the unsigned container profile.
	UnsignedPreserved,
	/// The source signature and any ticket were excluded from the unsigned output.
	SignatureAndAnyTicketRemoved,
}

/// Successful conversion and the checks that applied at that output boundary.
#[derive(Debug)]
#[non_exhaustive]
pub struct Report {
	pub source_profile: Profile,
	pub output_profile: Profile,
	pub input_checks: Verification,
	pub output: WriteReport,
	pub output_check: OutputCheck,
	pub trust: TrustState,
}

/// Conversion failed without producing a complete verified image.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("conversion target must be UDRO or UDZO")]
	Target,
	#[error("source image could not be admitted or verified")]
	Input(#[source] UdifError),
	#[error(
		"conversion requires explicit permission to discard the embedded signature and any ticket"
	)]
	SignatureLossNotAllowed,
	#[error("container encoding did not complete")]
	Encode(#[source] UdifError),
	#[error("staged container could not be reopened or verified")]
	Output(#[source] UdifError),
	#[error("staged container length disagrees with its write report")]
	OutputLength,
	#[error("staged container changed the qualified metadata or geometry")]
	OutputMetadata,
	#[error("staged logical bytes differ from the source at byte {offset}")]
	OutputMismatch { offset: u64 },
	#[error("conversion was cancelled")]
	Cancelled,
}

/// A path-based conversion failure. Publication failures retain the staging
/// error and any causal conversion error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FileError {
	#[error("could not open the input image")]
	Open(#[source] io::Error),
	#[error("could not admit the output path")]
	Destination(#[source] io::Error),
	#[error("destination already exists")]
	DestinationExists,
	#[error(transparent)]
	Prepare(#[from] Error),
	#[error(transparent)]
	Publish(#[from] PublishError<Error>),
}

/// Convert an immutable input into a caller-owned stream without a raw spool.
/// The caller provides a distinct sink. A failure after emission begins may
/// leave an invalid prefix, which the caller must discard.
/// # Errors
/// Refuses unsupported source framing, metadata, target profile or integrity
/// failures, exhausted bounds, cancellation and failed reads or writes.
pub fn stream(source: impl ReadAt, sink: impl Write, options: &Options) -> Result<Report, Error> {
	let (mut plan, input_checks) = prepare(source, options)?;
	let output = plan.encode(sink, options)?;
	Ok(report(
		plan.metadata,
		input_checks,
		output,
		options,
		OutputCheck::Writer,
		plan.trust,
	))
}

/// Convert a regular file into a new path, stage it in the destination
/// directory, and verify exact logical bytes before no-clobber publication.
/// The destination directory must remain stable throughout the operation.
/// # Errors
/// Refuses any existing destination, including an alias of the source.
/// Conversion, verification or publication failure leaves existing files intact.
pub fn file(source: &Path, destination: &Path, options: &Options) -> Result<Report, FileError> {
	let source = open_source(source, destination)?;
	let (plan, input_checks) = prepare(source, options)?;
	publish_plan(plan, input_checks, destination, options)
}

/// Convert a regular file to a new unsigned image with explicit signature policy.
///
/// The signed profile uses structural admission and container checksums. Signature
/// integrity, ticket validity and platform trust are separate from conversion.
/// The destination directory must remain stable throughout the operation.
///
/// # Errors
/// Refuses signed input without permission, malformed or unsupported envelopes,
/// failed checksums, and existing destinations. Publication requires exact logical
/// byte equality between the admitted source and the reopened unsigned stage.
pub fn file_with_signature_policy(
	source: &Path,
	destination: &Path,
	options: &Options,
	policy: SignaturePolicy,
) -> Result<Report, FileError> {
	let source = open_source(source, destination)?;
	let (plan, input_checks) = prepare_source(source, options, |source, limits| {
		let image = Image::open_release(source, limits).map_err(Error::Input)?;
		if policy == SignaturePolicy::RequireUnsigned && image.embedded_signature_range().is_some()
		{
			return Err(Error::SignatureLossNotAllowed);
		}
		Ok(image)
	})?;
	publish_plan(plan, input_checks, destination, options)
}

pub(crate) fn open_source(source: &Path, destination: &Path) -> Result<FileSource, FileError> {
	match fs::symlink_metadata(destination) {
		Ok(_) => return Err(FileError::DestinationExists),
		Err(error) if error.kind() == io::ErrorKind::NotFound => {}
		Err(error) => return Err(FileError::Destination(error)),
	}
	if !fs::metadata(source).map_err(FileError::Open)?.is_file() {
		return Err(Error::Input(UdifError::InvalidFormat(
			"conversion source must be a regular file",
		))
		.into());
	}
	Ok(FileSource::new(File::open(source).map_err(FileError::Open)?).map_err(Error::Input)?)
}

pub(crate) fn publish_plan<S: ReadAt>(
	plan: Plan<S>,
	input_checks: Verification,
	destination: &Path,
	options: &Options,
) -> Result<Report, FileError> {
	let metadata = plan.metadata;
	let trust = plan.trust;
	let plan = RefCell::new(plan);
	let output = publish_new(
		destination,
		|sink| plan.borrow_mut().encode(sink, options),
		|stage, output| plan.borrow_mut().validate(stage, output, options),
	)?;
	Ok(report(
		metadata,
		input_checks,
		output,
		options,
		OutputCheck::VerifiedAndCompared,
		trust,
	))
}

fn report(
	metadata: RebuildMetadata,
	input_checks: Verification,
	output: WriteReport,
	options: &Options,
	output_check: OutputCheck,
	trust: TrustState,
) -> Report {
	Report {
		source_profile: metadata.source_profile(),
		output_profile: options.output.profile,
		input_checks,
		output,
		output_check,
		trust,
	}
}

pub(crate) struct Plan<S> {
	image: Image<CheckedSource<S>>,
	metadata: RebuildMetadata,
	trust: TrustState,
}

pub(crate) fn prepare<S: ReadAt>(
	source: S,
	options: &Options,
) -> Result<(Plan<S>, Verification), Error> {
	prepare_source(source, options, |source, limits| {
		Image::open(source, limits).map_err(Error::Input)
	})
}

fn prepare_source<S: ReadAt>(
	source: S,
	options: &Options,
	admit: impl FnOnce(CheckedSource<S>, Limits) -> Result<Image<CheckedSource<S>>, Error>,
) -> Result<(Plan<S>, Verification), Error> {
	if !matches!(options.output.profile, Profile::Udro | Profile::Udzo) {
		return Err(Error::Target);
	}
	if options.output.cancellation.is_cancelled() {
		return Err(Error::Cancelled);
	}
	let available = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
	options.output.admit(available).map_err(Error::Encode)?;
	let source = CheckedSource {
		source,
		cancellation: options.output.cancellation.clone(),
	};
	let mut image = admit(source, options.input)?;
	let trust = if image.embedded_signature_range().is_some() {
		TrustState::SignatureAndAnyTicketRemoved
	} else {
		TrustState::UnsignedPreserved
	};
	let metadata = image.rebuild_release_metadata().map_err(Error::Input)?;
	options
		.output
		.max_data_runs(image.descriptor().logical_bytes)
		.map_err(Error::Encode)?;
	let checks = image.verify().map_err(Error::Input)?;
	check_source(&mut image)?;
	Ok((
		Plan {
			image,
			metadata,
			trust,
		},
		checks,
	))
}

impl<S: ReadAt> Plan<S> {
	pub(crate) fn logical_bytes(&self) -> u64 {
		self.image.descriptor().logical_bytes
	}

	pub(crate) fn read_exact_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
		self.image
			.read_exact_at(offset, bytes)
			.map_err(Error::Input)
	}

	fn encode(&mut self, sink: impl Write, options: &Options) -> Result<WriteReport, Error> {
		let reader = LogicalReader {
			image: &mut self.image,
			offset: 0,
		};
		let output = write_rebuilt_image(reader, sink, &options.output, self.metadata)
			.map_err(Error::Encode)?;
		check_source(&mut self.image)?;
		Ok(output)
	}

	fn validate(
		&mut self,
		mut stage: StagedReader<'_>,
		output: &WriteReport,
		options: &Options,
	) -> Result<(), Error> {
		let length = stage
			.seek(SeekFrom::End(0))
			.map_err(|error| Error::Output(error.into()))?;
		if length != output.physical_bytes {
			return Err(Error::OutputLength);
		}
		let source = StagedSource {
			stage: RefCell::new(stage),
			length,
		};
		let source = CheckedSource {
			source,
			cancellation: options.output.cancellation.clone(),
		};
		let limits = Limits {
			max_physical_bytes: options.output.max_physical_bytes,
			max_logical_bytes: options.output.max_logical_bytes,
			..Limits::default()
		};
		let mut staged = Image::open(source, limits).map_err(Error::Output)?;
		let metadata = staged.rebuild_metadata().map_err(Error::Output)?;
		if metadata.source_profile() != options.output.profile
			|| !self.metadata.matches_preserved_fields(&metadata)
		{
			return Err(Error::OutputMetadata);
		}
		staged.verify().map_err(Error::Output)?;
		compare_logical_bytes(&mut self.image, &mut staged)
	}
}

fn compare_logical_bytes(
	source: &mut Image<impl ReadAt>,
	staged: &mut Image<impl ReadAt>,
) -> Result<(), Error> {
	let logical_bytes = source.descriptor().logical_bytes;
	let mut original = vec![0; COMPARE_WINDOW_BYTES];
	let mut output = vec![0; COMPARE_WINDOW_BYTES];
	let mut offset = 0;
	while offset < logical_bytes {
		let size = (logical_bytes - offset).min(COMPARE_WINDOW_BYTES as u64) as usize;
		source
			.read_exact_at(offset, &mut original[..size])
			.map_err(Error::Input)?;
		staged
			.read_exact_at(offset, &mut output[..size])
			.map_err(Error::Output)?;
		if let Some(index) = original[..size]
			.iter()
			.zip(&output[..size])
			.position(|(a, b)| a != b)
		{
			return Err(Error::OutputMismatch {
				offset: offset + index as u64,
			});
		}
		offset += size as u64;
	}
	check_source(source)
}

fn check_source(image: &mut Image<impl ReadAt>) -> Result<(), Error> {
	image
		.read_exact_at(image.descriptor().logical_bytes, &mut [])
		.map_err(Error::Input)
}

struct LogicalReader<'image, S> {
	image: &'image mut Image<S>,
	offset: u64,
}

impl<S: ReadAt> Read for LogicalReader<'_, S> {
	fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
		let remaining = self.image.descriptor().logical_bytes - self.offset;
		let len = remaining.min(output.len() as u64) as usize;
		self.image
			.read_exact_at(self.offset, &mut output[..len])
			.map_err(io::Error::other)?;
		self.offset += len as u64;
		Ok(len)
	}
}

struct CheckedSource<S> {
	source: S,
	cancellation: Cancellation,
}

impl<S: ReadAt> ReadAt for CheckedSource<S> {
	fn len(&self) -> u64 {
		self.source.len()
	}

	fn check_unchanged(&self) -> Result<(), UdifError> {
		if self.cancellation.is_cancelled() {
			return Err(UdifError::Cancelled);
		}
		self.source.check_unchanged()
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), UdifError> {
		self.check_unchanged()?;
		self.source.read_exact_at(offset, output)
	}
}

struct StagedSource<'stage> {
	stage: RefCell<StagedReader<'stage>>,
	length: u64,
}

impl ReadAt for StagedSource<'_> {
	fn len(&self) -> u64 {
		self.length
	}

	fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), UdifError> {
		let len = output.len() as u64;
		if offset.checked_add(len).is_none_or(|end| end > self.length) {
			return Err(UdifError::OutOfRange {
				offset,
				len,
				disk_len: self.length,
			});
		}
		let mut stage = self.stage.borrow_mut();
		stage.seek(SeekFrom::Start(offset))?;
		stage.read_exact(output)?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use dmgez_udif::{SECTOR_BYTES, writer::write_image};

	use super::*;

	#[test]
	fn comparison_rejects_valid_images_with_different_logical_bytes() {
		const SIZE: usize = SECTOR_BYTES as usize;
		const CHANGE: usize = SIZE / 2;
		let original = [0; SIZE];
		let mut changed = original;
		changed[CHANGE] = 1;
		let wrap = |raw: &[u8]| {
			let mut output = Vec::new();
			write_image(raw, raw.len() as u64, &mut output, &WriteOptions::default()).unwrap();
			output
		};
		let original = wrap(&original);
		let changed = wrap(&changed);
		let mut source = Image::open(original.as_slice(), Limits::default()).unwrap();
		let mut staged = Image::open(changed.as_slice(), Limits::default()).unwrap();
		source.verify().unwrap();
		staged.verify().unwrap();
		assert!(
			matches!(compare_logical_bytes(&mut source, &mut staged), Err(Error::OutputMismatch { offset }) if offset == CHANGE as u64)
		);
	}
}
