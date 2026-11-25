use std::{
	fs::File,
	io,
	path::{Path, PathBuf},
};

use dmgez_macho::Classification;
use rustix::fs::{Mode, OFlags, open};
use thiserror::Error;

pub(super) use dmgez_macho::CodeKind as Kind;

#[derive(Debug, Error)]
pub(super) enum Error {
	#[error("could not read possible Mach-O code {path:?}")]
	Read {
		path: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("unsupported or ambiguous Mach-O code at {0:?}")]
	Unsupported(PathBuf),
}

pub(super) fn classify(path: &Path) -> Result<Option<Kind>, Error> {
	let mut file = open_file(path)?;
	match dmgez_macho::classify(&mut file).map_err(|source| read_error(path, source))? {
		Classification::Code(kind) => Ok(Some(kind)),
		Classification::Other => Ok(None),
		_ => Err(Error::Unsupported(path.to_path_buf())),
	}
}

pub(super) fn architectures(path: &Path) -> Result<Vec<&'static str>, Error> {
	let mut file = open_file(path)?;
	match dmgez_macho::classify(&mut file).map_err(|source| read_error(path, source))? {
		Classification::Other => Ok(Vec::new()),
		Classification::Code(_) => dmgez_macho::architectures(&mut file)
			.map_err(|source| read_error(path, source))?
			.map(|list| {
				list.into_iter()
					.map(dmgez_macho::Architecture::as_str)
					.collect()
			})
			.ok_or_else(|| Error::Unsupported(path.to_path_buf())),
		_ => Err(Error::Unsupported(path.to_path_buf())),
	}
}

fn open_file(path: &Path) -> Result<File, Error> {
	let file = open(
		path,
		OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
		Mode::empty(),
	)
	.map(File::from)
	.map_err(|source| read_error(path, source.into()))?;
	if file
		.metadata()
		.map_err(|source| read_error(path, source))?
		.is_file()
	{
		Ok(file)
	} else {
		Err(Error::Unsupported(path.to_path_buf()))
	}
}

fn read_error(path: &Path, source: io::Error) -> Error {
	Error::Read {
		path: path.to_path_buf(),
		source,
	}
}
