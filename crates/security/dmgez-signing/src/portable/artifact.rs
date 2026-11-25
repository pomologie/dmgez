use std::{
	fs::{self, File, Metadata},
	io::{self, Read},
	path::{Path, PathBuf},
};

use dmgez_core::source::CapturedFile;
use dmgez_udif::{MAX_EMBEDDED_SIGNATURE_BYTES, TRAILER_BYTES, check_trailer_header};
use same_file::Handle;

use crate::{ArtifactProblem, Error, SignError};

use super::quarantine;

const IMAGE_EXTENSION: &str = "dmg";
const MAX_IMAGE_BYTES: u64 =
	u32::MAX as u64 + (TRAILER_BYTES + MAX_EMBEDDED_SIGNATURE_BYTES) as u64;
const CHANGED: &str = "named entry differs from the held file";

#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const FILE_SHARE_DELETE: u32 = 0x0000_0004;

pub(super) struct Artifact {
	path: PathBuf,
	identity: Handle,
	bytes: CapturedFile,
}

impl Artifact {
	pub(super) fn open(path: &Path) -> Result<Self, SignError> {
		let path = image_path(path).map_err(|source| artifact_io(path, source))?;
		let file =
			open_regular(&path, Access::Capture).map_err(|source| artifact_io(&path, source))?;
		Self::from_file(&path, file)
	}

	pub(super) fn from_file(path: &Path, file: File) -> Result<Self, SignError> {
		let bytes = CapturedFile::new(file.try_clone().map_err(SignError::ImageIo)?)
			.map_err(source_error)?;
		if !(TRAILER_BYTES as u64..=MAX_IMAGE_BYTES).contains(&bytes.len()) {
			return Err(SignError::ImageSize);
		}
		let identity = Handle::from_file(file).map_err(SignError::ImageIo)?;
		let artifact = Self {
			path: path.to_path_buf(),
			identity,
			bytes,
		};
		artifact.validate()?;
		Ok(artifact)
	}

	pub(super) fn path(&self) -> &Path {
		&self.path
	}

	pub(super) fn file(&self) -> &File {
		self.identity.as_file()
	}

	pub(super) fn len(&self) -> u64 {
		self.bytes.len()
	}

	pub(super) fn trailer(&self) -> Result<[u8; TRAILER_BYTES], SignError> {
		let mut trailer = [0; TRAILER_BYTES];
		self.bytes
			.read_exact_at(self.len() - TRAILER_BYTES as u64, &mut trailer)
			.map_err(source_error)?;
		check_trailer_header(&trailer).map_err(|_| Error::InvalidArtifact {
			path: self.path.clone(),
			problem: ArtifactProblem::UdifHeader,
		})?;
		Ok(trailer)
	}

	pub(super) fn reader(&self) -> impl Read + '_ {
		self.bytes.reader()
	}

	pub(super) fn quarantine(&self) -> Result<Option<Vec<u8>>, SignError> {
		let value = quarantine::read(self.file(), self.path())?;
		self.validate()?;
		Ok(value)
	}

	pub(super) fn validate(&self) -> Result<(), SignError> {
		self.bytes.validate().map_err(source_error)?;
		check_file_name(&self.identity, &self.path).map_err(|source| {
			if matches!(
				source.kind(),
				io::ErrorKind::NotFound | io::ErrorKind::InvalidData
			) {
				Error::ArtifactChanged {
					path: self.path.clone(),
				}
			} else {
				artifact_io(&self.path, source)
			}
		})?;
		self.bytes.validate().map_err(source_error)
	}
}

pub(super) fn image_path(input: &Path) -> io::Result<PathBuf> {
	let leaf = input
		.file_name()
		.filter(|leaf| {
			Path::new(leaf)
				.extension()
				.is_some_and(|extension| extension.eq_ignore_ascii_case(IMAGE_EXTENSION))
		})
		.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected a .dmg filename"))?;
	#[cfg(windows)]
	if leaf.as_encoded_bytes().contains(&b':') {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"disk image must name a file without an alternate stream",
		));
	}
	let parent = input
		.parent()
		.filter(|path| !path.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	Ok(parent.canonicalize()?.join(leaf))
}

pub(super) fn check_file_name(held: &Handle, path: &Path) -> io::Result<()> {
	let named = Handle::from_file(open_regular(path, Access::Observe)?)?;
	if &named == held {
		Ok(())
	} else {
		Err(io::Error::new(io::ErrorKind::InvalidData, CHANGED))
	}
}

enum Access {
	Capture,
	Observe,
}

fn open_regular(path: &Path, access: Access) -> io::Result<File> {
	if !is_regular(&fs::symlink_metadata(path)?) {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			"expected a regular file",
		));
	}
	let file = open_read(path, access)?;
	if !is_regular(&file.metadata()?) {
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			"expected an open regular file",
		));
	}
	Ok(file)
}

#[cfg(unix)]
fn open_read(path: &Path, _: Access) -> io::Result<File> {
	use rustix::fs::{Mode, OFlags, open};

	let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
	Ok(File::from(open(path, flags, Mode::empty())?))
}

#[cfg(windows)]
fn open_read(path: &Path, access: Access) -> io::Result<File> {
	use std::os::windows::fs::OpenOptionsExt;

	let sharing = match access {
		Access::Capture => FILE_SHARE_READ,
		Access::Observe => FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
	};
	fs::OpenOptions::new()
		.read(true)
		.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
		.share_mode(sharing)
		.open(path)
}

fn is_regular(metadata: &Metadata) -> bool {
	#[cfg(windows)]
	{
		use std::os::windows::fs::MetadataExt;

		const FILE_ATTRIBUTE_DEVICE: u32 = 0x0000_0040;
		const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
		if metadata.file_attributes() & (FILE_ATTRIBUTE_DEVICE | FILE_ATTRIBUTE_REPARSE_POINT) != 0
		{
			return false;
		}
	}
	metadata.is_file()
}

fn artifact_io(path: &Path, source: io::Error) -> Error {
	Error::ArtifactIo {
		path: path.to_path_buf(),
		source,
	}
}

fn source_error(error: dmgez_core::source::SourceError) -> SignError {
	use dmgez_core::source::SourceError;

	match error {
		SourceError::Changed => SignError::ImageChanged,
		SourceError::Io(error) => SignError::ImageIo(error),
		error => SignError::ImageIo(io::Error::other(error)),
	}
}
