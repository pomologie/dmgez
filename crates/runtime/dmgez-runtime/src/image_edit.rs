//! Rebuild an admitted HFS+ or HFSX image with verified no-clobber publication.

use std::{
	cell::RefCell,
	fs,
	io::{self, Read, Write},
	path::{Path, absolute},
};

use dmgez_config::{
	MetadataPolicy,
	project_edit::{EditBatchError, MAX_EDIT_BATCH_BYTES, parse_edit_batch},
};
use dmgez_core::output::{PublishError, StagedReader, publish_new};
use dmgez_hfsplus::browse::{Limits, with_catalog};
use dmgez_hfsplus::{
	HostCopyMetadataLoss,
	rebuild::{PrepareError, PreparedEdit, seal_prepared, verify_rebuilt_volume},
};
use dmgez_operations::edit::image_source::{
	ImageSource, ImageSourceError, ImageSourceOptions, SourceEnvelope as ObservedEnvelope,
};
use dmgez_udif::writer::{RebuildMetadata, WriteOptions, WriteReport, write_rebuilt_image};
use dmgez_vfs::{Disk, Filesystem, Layout, VolumeId};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use dmgez_operations::edit::image_source::ImageRevision;

pub use crate::edit_input::encode_sha256;
use crate::{
	edit_input::{SHA256_BYTES, SHA256_HEX_BYTES, Snapshot},
	staged_image::{invalid, open_verified_stage},
};

const OUTPUT_EXISTS: &str = "image edit destination already exists";
const OUTPUT_FILENAME: &str = "the image edit destination has no file name";
const OUTPUT_DIRECTORY: &str = "the image edit destination parent is not a directory";
const MISSING_ENCODED_STAGE: &str = "the staged rebuild has no encoding report";
const METADATA_MISMATCH: &str = "the staged image differs from its admitted container metadata";
const LOGICAL_MISMATCH: &str = "the staged image does not contain the sealed logical bytes";
const STAGED_LAYOUT: &str = "the rebuilt filesystem differs from the admitted whole-disk layout";
const HEX_DIGITS_PER_BYTE: usize = 2;
const HEX_RADIX: u32 = 16;

/// Whether to validate a rebuild plan or publish a verified unsigned image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditMode {
	/// Plan the rebuild and revalidate its inputs without staging output.
	Plan,
	/// Rebuild, verify, and publish to the previously absent path.
	Publish,
}

/// Local inputs and publication intent for one copy-on-write image edit.
#[derive(Debug)]
pub struct EditRequest<'input> {
	/// Regular source file containing the qualified HFS+ or HFSX image.
	pub image: &'input Path,
	/// Bounded TOML edit batch. Host copies resolve relative to its canonical path.
	pub changes: &'input Path,
	/// Absent destination under a directory that stays stable during the operation.
	pub output: &'input Path,
	/// Optional required SHA-256 of the complete source image.
	pub expected_revision: Option<ImageRevision>,
	/// Fidelity policy for new host copies. Retained image entries use strict preservation.
	pub metadata_policy: MetadataPolicy,
	/// Whether to publish the rebuilt image.
	pub mode: EditMode,
}

/// Framing and integrity observed on the original image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEnvelope {
	/// The source image did not contain an embedded signature.
	Unsigned,
	/// The embedded signature's CodeDirectory digests matched.
	SignedDigestsVerified,
}

/// Facts about the validated edit and its optional publication.
#[derive(Debug)]
pub struct EditReport {
	/// SHA-256 of every byte of the original file, including any signature.
	pub revision: ImageRevision,
	/// How the original file was framed and checked.
	pub source_envelope: SourceEnvelope,
	/// Number of edits in the admitted batch.
	pub edits: usize,
	/// Number of entries in the rebuilt catalog.
	pub entries: usize,
	/// Fidelity losses observed in the capturing of new host copies.
	pub metadata_losses: Vec<HostCopyMetadataLoss>,
	/// Size of the rebuilt logical disk.
	pub logical_bytes: u64,
	/// Evidence from the verified unsigned output, when publication succeeded.
	pub publication: Option<Publication>,
}

/// Size and decoded byte revision of a successfully published unsigned image.
#[derive(Debug)]
pub struct Publication {
	/// Size of the complete published UDIF file.
	pub physical_bytes: u64,
	/// SHA-256 shared by the sealed logical disk and the decoded output.
	pub logical_sha256: [u8; SHA256_BYTES],
}

/// Failure to prepare, validate, or publish one copy-on-write edit.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EditError {
	#[error("expected image SHA-256 must contain 64 hexadecimal digits")]
	InvalidRevision,
	#[error("the image uses an unsupported source envelope")]
	UnsupportedEnvelope,
	#[error("the edit batch has no parent directory")]
	ChangesDirectory,
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Batch(#[from] EditBatchError),
	#[error(transparent)]
	Source(#[from] ImageSourceError),
	#[error(transparent)]
	Prepare(#[from] PrepareError),
	#[error(transparent)]
	Publish(#[from] PublishError<io::Error>),
}

/// Decode the 64 hexadecimal digits used to identify an exact source revision.
///
/// # Errors
/// Rejects any other length or encoding.
pub fn parse_expected_revision(value: &str) -> Result<ImageRevision, EditError> {
	if value.len() != SHA256_HEX_BYTES || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(EditError::InvalidRevision);
	}
	let mut digest = [0; SHA256_BYTES];
	for (index, byte) in digest.iter_mut().enumerate() {
		let start = index * HEX_DIGITS_PER_BYTE;
		*byte = u8::from_str_radix(&value[start..start + HEX_DIGITS_PER_BYTE], HEX_RADIX)
			.map_err(|_| EditError::InvalidRevision)?;
	}
	Ok(ImageRevision::from_sha256(digest))
}

/// Apply the edit batch to a read-only source and optionally publish a new image.
///
/// # Errors
/// Rejects invalid inputs, unsupported profiles, changed sources, occupied
/// destinations, failed read-back checks, and I/O errors.
pub fn edit(request: EditRequest<'_>) -> Result<EditReport, EditError> {
	let output = absolute(request.output)?;
	check_destination(&output)?;
	let changes = Snapshot::open(request.changes, MAX_EDIT_BATCH_BYTES)?;
	let changes_dir = changes.path.parent().ok_or(EditError::ChangesDirectory)?;
	let edits = parse_edit_batch(&changes.text)?;
	let mut source = ImageSource::open(
		request.image,
		request.expected_revision,
		ImageSourceOptions::default(),
	)?;
	let revision = source.revision();
	let source_envelope = match source.envelope() {
		ObservedEnvelope::Unsigned => SourceEnvelope::Unsigned,
		ObservedEnvelope::SignedDigestsVerified => SourceEnvelope::SignedDigestsVerified,
		_ => return Err(EditError::UnsupportedEnvelope),
	};
	let prepared = PreparedEdit::new(edits, changes_dir, &output, request.metadata_policy)?;
	let rebuilt = match request.mode {
		EditMode::Plan => {
			let plan = rebuild(&mut source, &prepared, None)?;
			validate_inputs(&source, &changes, &prepared)?;
			plan
		}
		EditMode::Publish => {
			let source = RefCell::new(source);
			publish_new(
				&output,
				|sink| {
					let mut source = source.try_borrow_mut().map_err(io::Error::other)?;
					rebuild(&mut source, &prepared, Some(sink))
				},
				|stage, rebuilt| {
					verify(stage, rebuilt)?;
					let source = source.try_borrow().map_err(io::Error::other)?;
					validate_inputs(&source, &changes, &prepared)
				},
			)?
		}
	};
	Ok(EditReport {
		revision,
		source_envelope,
		edits: prepared.edits_count(),
		entries: rebuilt.entries,
		metadata_losses: prepared.into_metadata_losses(),
		logical_bytes: rebuilt.logical_bytes,
		publication: rebuilt.encoded.map(|encoded| Publication {
			physical_bytes: encoded.written.physical_bytes,
			logical_sha256: encoded.logical_sha256,
		}),
	})
}

fn validate_inputs(
	source: &ImageSource,
	changes: &Snapshot,
	prepared: &PreparedEdit,
) -> io::Result<()> {
	source.revalidate().map_err(io::Error::other)?;
	changes.validate()?;
	prepared.validate_sources().map_err(io::Error::other)
}

fn check_destination(path: &Path) -> io::Result<()> {
	if path.file_name().is_none() {
		return Err(io::Error::new(io::ErrorKind::InvalidInput, OUTPUT_FILENAME));
	}
	let parent = path
		.parent()
		.filter(|candidate| !candidate.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	if !fs::metadata(parent)?.is_dir() {
		return Err(io::Error::new(
			io::ErrorKind::NotADirectory,
			OUTPUT_DIRECTORY,
		));
	}
	match fs::symlink_metadata(path) {
		Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, OUTPUT_EXISTS)),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(error),
	}
}

struct Rebuilt {
	entries: usize,
	logical_bytes: u64,
	filesystem: Filesystem,
	encoded: Option<Encoded>,
}

struct Encoded {
	written: WriteReport,
	metadata: RebuildMetadata,
	logical_sha256: [u8; SHA256_BYTES],
}

fn rebuild(
	source: &mut ImageSource,
	prepared: &PreparedEdit,
	sink: Option<&mut dyn Write>,
) -> io::Result<Rebuilt> {
	let source_metadata = source.container_metadata();
	let filesystem = source.filesystem();
	let mut disk = Disk::discover(source, dmgez_vfs::Limits::default()).map_err(invalid)?;
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		Limits::default(),
		|volume, catalog| {
			let plan = seal_prepared(volume, &catalog, prepared).map_err(invalid)?;
			let logical_bytes = plan.logical_bytes();
			let options = WriteOptions {
				profile: source_metadata.source_profile(),
				..WriteOptions::default()
			};
			options.max_data_runs(logical_bytes).map_err(invalid)?;
			let encoded = if let Some(sink) = sink {
				let mut digest = Sha256::new();
				let reader = HashReader {
					reader: plan.reader(),
					digest: &mut digest,
				};
				let metadata = source_metadata.with_logical_bytes(logical_bytes);
				let written =
					write_rebuilt_image(reader, sink, &options, metadata).map_err(invalid)?;
				Some(Encoded {
					written,
					metadata,
					logical_sha256: digest.finalize().into(),
				})
			} else {
				None
			};
			Ok(Rebuilt {
				entries: plan.entries(),
				logical_bytes,
				filesystem,
				encoded,
			})
		},
	)
	.map_err(invalid)?
}

fn verify(stage: StagedReader<'_>, rebuilt: &Rebuilt) -> io::Result<()> {
	let encoded = rebuilt
		.encoded
		.as_ref()
		.ok_or_else(|| invalid(MISSING_ENCODED_STAGE))?;
	let mut image = open_verified_stage(stage, &encoded.written)?;
	let actual = image.rebuild_metadata().map_err(invalid)?;
	if !encoded.metadata.matches_preserved_fields(&actual)
		|| encoded.metadata.source_profile() != actual.source_profile()
	{
		return Err(invalid(METADATA_MISMATCH));
	}
	let mut sink = HashSink(Sha256::new());
	image.write_raw_to(&mut sink).map_err(invalid)?;
	let actual_digest: [u8; SHA256_BYTES] = sink.0.finalize().into();
	if actual_digest != encoded.logical_sha256 {
		return Err(invalid(LOGICAL_MISMATCH));
	}
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).map_err(invalid)?;
	if !matches!(
		disk.layout(),
		Layout::Partitionless { filesystem, range }
			if *filesystem == rebuilt.filesystem && *range == (0..rebuilt.logical_bytes)
	) {
		return Err(invalid(STAGED_LAYOUT));
	}
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		Limits::default(),
		|volume, catalog| verify_rebuilt_volume(volume, &catalog).map_err(invalid),
	)
	.map_err(invalid)?
}

struct HashReader<'digest, R> {
	reader: R,
	digest: &'digest mut Sha256,
}

impl<R: Read> Read for HashReader<'_, R> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let received = self.reader.read(buffer)?;
		self.digest.update(&buffer[..received]);
		Ok(received)
	}
}

struct HashSink(Sha256);

impl Write for HashSink {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.0.update(bytes);
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}
