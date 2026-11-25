use std::{
	fs::{self, File, Metadata},
	io::{self, Read, Seek, SeekFrom},
	os::unix::fs::MetadataExt,
	path::{Path, PathBuf},
};

use dmgez_udif::{TRAILER_BYTES, check_trailer_header};
use rustix::fs::{Mode, OFlags, open};

use crate::{ArtifactKind, ArtifactProblem, Error};

const APP_EXTENSION: &str = "app";
const DMG_EXTENSION: &str = "dmg";

pub(super) struct Artifact {
	path: PathBuf,
	kind: ArtifactKind,
	initial: Metadata,
	// Keep the inode alive while the system tool opens the path independently.
	_file: File,
}

impl Artifact {
	pub(super) fn open(input: &Path) -> Result<Self, Error> {
		let path = input
			.canonicalize()
			.map_err(|source| io_error(input, source))?;
		let extension = path.extension().unwrap_or_default();
		let initial = fs::symlink_metadata(&path).map_err(|source| io_error(&path, source))?;
		let (kind, kind_flags) =
			if extension.eq_ignore_ascii_case(APP_EXTENSION) && initial.is_dir() {
				(ArtifactKind::AppBundle, OFlags::DIRECTORY)
			} else if extension.eq_ignore_ascii_case(DMG_EXTENSION) && initial.is_file() {
				(ArtifactKind::DiskImage, OFlags::empty())
			} else {
				return Err(Error::InvalidArtifact {
					path,
					problem: ArtifactProblem::Kind,
				});
			};
		let flags =
			OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | kind_flags;
		let fd =
			open(&path, flags, Mode::empty()).map_err(|source| io_error(&path, source.into()))?;
		let mut file = File::from(fd);
		let opened = file.metadata().map_err(|source| io_error(&path, source))?;
		if !same_snapshot(&initial, &opened) {
			return Err(Error::ArtifactChanged { path });
		}
		if kind == ArtifactKind::DiskImage
			&& !is_udif(&mut file, opened.len()).map_err(|source| io_error(&path, source))?
		{
			return Err(Error::InvalidArtifact {
				path,
				problem: ArtifactProblem::UdifHeader,
			});
		}
		Ok(Self {
			path,
			kind,
			initial,
			_file: file,
		})
	}

	pub(super) fn path(&self) -> &Path {
		&self.path
	}

	pub(super) fn kind(&self) -> ArtifactKind {
		self.kind
	}

	pub(super) fn invalid(&self, problem: ArtifactProblem) -> Error {
		Error::InvalidArtifact {
			path: self.path.clone(),
			problem,
		}
	}

	pub(super) fn validate(&self) -> Result<(), Error> {
		match fs::symlink_metadata(&self.path) {
			Ok(current) if same_snapshot(&self.initial, &current) => Ok(()),
			Err(error) if error.kind() != io::ErrorKind::NotFound => {
				Err(io_error(&self.path, error))
			}
			_ => Err(Error::ArtifactChanged {
				path: self.path.clone(),
			}),
		}
	}
}

fn is_udif(file: &mut File, length: u64) -> io::Result<bool> {
	if length < TRAILER_BYTES as u64 {
		return Ok(false);
	}
	file.seek(SeekFrom::End(-(TRAILER_BYTES as i64)))?;
	let mut bytes = [0; TRAILER_BYTES];
	file.read_exact(&mut bytes)?;
	Ok(check_trailer_header(&bytes).is_ok())
}

fn io_error(path: &Path, source: io::Error) -> Error {
	Error::ArtifactIo {
		path: path.to_path_buf(),
		source,
	}
}

fn same_snapshot(left: &Metadata, right: &Metadata) -> bool {
	left.dev() == right.dev()
		&& left.ino() == right.ino()
		&& left.mode() == right.mode()
		&& left.len() == right.len()
		&& left.mtime() == right.mtime()
		&& left.mtime_nsec() == right.mtime_nsec()
		&& left.ctime() == right.ctime()
		&& left.ctime_nsec() == right.ctime_nsec()
}
