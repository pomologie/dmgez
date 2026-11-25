//! Admission and exact revision checks for a read-only image edit source.

use std::{
	fs::{self, File},
	io,
	ops::Range,
	path::{Path, PathBuf},
	sync::Arc,
};

use dmgez_udif::{
	Cancellation, Error as UdifError, MAX_EMBEDDED_SIGNATURE_BYTES, SignatureIntegrityError,
	TRAILER_BYTES,
	reader::{CheckStatus, FileSource, Image, Limits, ReadAt},
	verify_code_directory_digests,
	writer::RebuildMetadata,
};
use dmgez_vfs::{Disk, Filesystem, Layout, Limits as DiskLimits, LogicalDisk};
use sha2::{Digest, Sha256};
use thiserror::Error;

const HASH_BUFFER_BYTES: usize = 64 << 10;
const SHA256_BYTES: usize = 32;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 1;

/// Exact SHA-256 over the entire input, including any signature and trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageRevision([u8; SHA256_BYTES]);

impl ImageRevision {
	/// Wraps an expected full-file SHA-256 digest.
	#[must_use]
	pub const fn from_sha256(digest: [u8; SHA256_BYTES]) -> Self {
		Self(digest)
	}

	/// The digest bytes in SHA-256 output order.
	#[must_use]
	pub const fn sha256(&self) -> &[u8; SHA256_BYTES] {
		&self.0
	}
}

/// Observed source framing and integrity. This carries no authenticity,
/// notarization, or operating-system trust decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceEnvelope {
	/// The admitted image has no embedded signature frame.
	Unsigned,
	/// The structurally admitted CodeDirectory's SHA-256 digests matched.
	SignedDigestsVerified,
}

/// Admission and cancellation controls shared by the complete source view.
#[derive(Debug, Default, Clone)]
pub struct ImageSourceOptions {
	/// Bounds on container size, logical geometry and decoded metadata.
	pub container: Limits,
	/// Bounds on partition and whole-disk layout discovery.
	pub disk: DiskLimits,
	/// Cancellation shared by admission, logical reads and revalidation.
	pub cancellation: Cancellation,
}

/// Failure to admit or revalidate an existing image for an edit.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImageSourceError {
	#[error("could not open or observe the input image")]
	Open(#[from] io::Error),
	#[error("image edit source must be a regular file")]
	NotRegular,
	#[error("the image source path no longer names the admitted file")]
	PathChanged,
	#[error("image SHA-256 differs from the required revision")]
	RevisionMismatch,
	#[error("input image does not use the qualified container profile")]
	Container(#[from] UdifError),
	#[error("input image signature digests could not be verified")]
	Signature(#[from] SignatureIntegrityError),
	#[error("input image partition layout could not be read")]
	Layout(#[from] dmgez_vfs::Error),
	#[error("image editing requires one HFS+ volume covering the whole logical disk")]
	NotWholeHfsPlus,
	#[error("input image is missing one or more required CRC32 checksums")]
	MissingChecksums,
}

/// An admitted HFS+ image and its pinned, read-only input handle.
///
/// Admission establishes container layout and integrity. The filesystem
/// adapter must still qualify the volume and account for metadata before
/// allowing a rebuild. The caller must keep the source stable during reads
/// and call [`Self::revalidate`] at the publication boundary.
#[derive(Debug)]
pub struct ImageSource {
	path: PathBuf,
	file: PinnedFile,
	#[cfg(unix)]
	identity: std::fs::Metadata,
	revision: ImageRevision,
	envelope: SourceEnvelope,
	metadata: RebuildMetadata,
	logical: Image<PinnedFile>,
}

impl ImageSource {
	/// Admit a signed or unsigned UDRO/UDZO image whose logical disk is one
	/// complete partitionless HFS+ volume. When supplied, the expected
	/// revision must match all physical input bytes.
	/// # Errors
	/// Rejects non-regular sources, unsupported layouts or metadata, failed
	/// checksums, source changes, cancellation and digest mismatches.
	pub fn open(
		path: &Path,
		expected: Option<ImageRevision>,
		options: ImageSourceOptions,
	) -> Result<Self, ImageSourceError> {
		let path = std::path::absolute(path)?;
		let opened = open_regular(&path)?;
		#[cfg(unix)]
		let identity = opened.metadata()?;
		let file = PinnedFile {
			source: Arc::new(FileSource::new(opened)?),
			cancellation: options.cancellation,
		};
		let mut logical = Image::open_release(file.clone(), options.container)?;
		let metadata = logical.rebuild_release_metadata()?;
		let logical_bytes = logical.descriptor().logical_bytes;
		let disk = Disk::discover(&mut logical, options.disk)?;
		if !matches!(
			disk.layout(),
			Layout::Partitionless { filesystem: Filesystem::HfsPlus, range }
				if *range == (0..logical_bytes)
		) {
			return Err(ImageSourceError::NotWholeHfsPlus);
		}
		let signature = logical.embedded_signature_range();
		let (revision, envelope) = source_revision(&file, signature)?;
		if expected.is_some_and(|expected| expected != revision) {
			return Err(ImageSourceError::RevisionMismatch);
		}
		let checks = logical.verify_with_cancellation(&file.cancellation)?;
		if checks.data_fork != CheckStatus::Passed
			|| checks.master != CheckStatus::Passed
			|| checks
				.tables
				.iter()
				.any(|status| *status != CheckStatus::Passed)
		{
			return Err(ImageSourceError::MissingChecksums);
		}
		let source = Self {
			path,
			file,
			#[cfg(unix)]
			identity,
			revision,
			envelope,
			metadata,
			logical,
		};
		source.revalidate_path()?;
		source.file.check_unchanged()?;
		Ok(source)
	}

	#[must_use]
	pub const fn revision(&self) -> ImageRevision {
		self.revision
	}

	#[must_use]
	pub const fn envelope(&self) -> SourceEnvelope {
		self.envelope
	}

	/// Container metadata admitted for regeneration independently of filesystem
	/// metadata. The fixed-geometry writer requires the original logical length.
	#[must_use]
	pub const fn container_metadata(&self) -> RebuildMetadata {
		self.metadata
	}

	/// Recheck the named entry and recompute the exact held source revision.
	/// The named check compares device and inode on Unix and reopens a matching
	/// byte revision on Windows, where the held source denies write and delete sharing.
	/// # Errors
	/// Fails if the source path changed, the held file changed, the expected
	/// bytes cannot be read, or cancellation was requested.
	pub fn revalidate(&self) -> Result<(), ImageSourceError> {
		let (observed, _) = source_revision(&self.file, None)?;
		if observed != self.revision {
			return Err(ImageSourceError::RevisionMismatch);
		}
		self.revalidate_path()?;
		self.file.check_unchanged()?;
		Ok(())
	}

	#[cfg(unix)]
	fn revalidate_path(&self) -> Result<(), ImageSourceError> {
		use std::os::unix::fs::MetadataExt;

		let named = fs::symlink_metadata(&self.path)?;
		if !named.is_file()
			|| (named.dev(), named.ino()) != (self.identity.dev(), self.identity.ino())
		{
			return Err(ImageSourceError::PathChanged);
		}
		Ok(())
	}

	#[cfg(not(unix))]
	fn revalidate_path(&self) -> Result<(), ImageSourceError> {
		// The portable metadata view has no stable named-entry identity. Check
		// the exact bytes through a fresh handle, under the admitted byte limit.
		let named = PinnedFile {
			source: Arc::new(FileSource::new(open_regular(&self.path)?)?),
			cancellation: self.file.cancellation.clone(),
		};
		if named.len() != self.file.len() || source_revision(&named, None)?.0 != self.revision {
			return Err(ImageSourceError::PathChanged);
		}
		Ok(())
	}
}

impl LogicalDisk for ImageSource {
	fn len(&self) -> u64 {
		self.logical.descriptor().logical_bytes
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), UdifError> {
		self.logical.read_exact_at(offset, output)
	}
}

#[derive(Debug, Clone)]
struct PinnedFile {
	source: Arc<FileSource>,
	cancellation: Cancellation,
}

impl ReadAt for PinnedFile {
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
		if self.cancellation.is_cancelled() {
			return Err(UdifError::Cancelled);
		}
		self.source.read_exact_at(offset, output)
	}
}

fn open_regular(path: &Path) -> Result<File, ImageSourceError> {
	if !is_regular(&fs::symlink_metadata(path)?) {
		return Err(ImageSourceError::NotRegular);
	}
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	let opened = {
		use rustix::fs::{Mode, OFlags, open};

		let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
		File::from(open(path, flags, Mode::empty()).map_err(io::Error::from)?)
	};
	#[cfg(windows)]
	let opened = {
		use std::os::windows::fs::OpenOptionsExt;

		File::options()
			.read(true)
			.share_mode(FILE_SHARE_READ)
			.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
			.open(path)?
	};
	#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
	let opened = File::open(path)?;
	if !is_regular(&opened.metadata()?) {
		return Err(ImageSourceError::NotRegular);
	}
	Ok(opened)
}

fn is_regular(metadata: &fs::Metadata) -> bool {
	#[cfg(windows)]
	{
		use std::os::windows::fs::MetadataExt;

		if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
			return false;
		}
	}
	metadata.is_file()
}

fn source_revision(
	file: &PinnedFile,
	signature: Option<Range<u64>>,
) -> Result<(ImageRevision, SourceEnvelope), ImageSourceError> {
	let body_end = signature.as_ref().map_or(file.len(), |range| range.start);
	let mut hash = Sha256::new();
	let mut buffer = [0; HASH_BUFFER_BYTES];
	hash_range(file, &mut hash, &mut buffer, 0..body_end)?;
	let envelope = if let Some(signature) = signature {
		verify_signature(file, signature, &hash.clone().finalize().into())?;
		hash_range(file, &mut hash, &mut buffer, body_end..file.len())?;
		SourceEnvelope::SignedDigestsVerified
	} else {
		SourceEnvelope::Unsigned
	};
	file.check_unchanged()?;
	Ok((ImageRevision::from_sha256(hash.finalize().into()), envelope))
}

fn hash_range(
	file: &PinnedFile,
	hash: &mut Sha256,
	buffer: &mut [u8],
	range: Range<u64>,
) -> Result<(), UdifError> {
	let mut offset = range.start;
	while offset < range.end {
		let size = (range.end - offset).min(buffer.len() as u64) as usize;
		let chunk = &mut buffer[..size];
		file.read_exact_at(offset, chunk)?;
		hash.update(chunk);
		offset += size as u64;
	}
	Ok(())
}

fn verify_signature(
	file: &PinnedFile,
	range: Range<u64>,
	body_digest: &[u8; SHA256_BYTES],
) -> Result<(), ImageSourceError> {
	let size = usize::try_from(range.end - range.start)
		.ok()
		.filter(|size| *size <= MAX_EMBEDDED_SIGNATURE_BYTES)
		.ok_or(UdifError::ResourceLimit("UDIF embedded signature bytes"))?;
	let mut frame = Vec::new();
	frame.try_reserve_exact(size).map_err(io::Error::other)?;
	frame.resize(size, 0);
	file.read_exact_at(range.start, &mut frame)?;
	let mut trailer = [0; TRAILER_BYTES];
	file.read_exact_at(range.end, &mut trailer)?;
	verify_code_directory_digests(&frame, &trailer, range.start, body_digest)?;
	Ok(())
}
