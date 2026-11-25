use std::{
	ffi::OsStr,
	fs::{File, OpenOptions},
	io,
	os::windows::{ffi::OsStrExt, fs::OpenOptionsExt as _},
	path::{Component, Path, PathBuf, Prefix},
};

use fs_at::os::windows::OpenOptionsExt as _;
use windows_namespace_request_sys::{
	FinalPathFlags, QueryFileInformationByHandle, QueryFinalPath, QueryVolumeInformation,
	file_basic::QueryFileBasicInfo,
};
use windows_sys::Win32::Storage::FileSystem::{
	FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
	FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
	FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_TRAVERSE,
};

use super::{
	EntryKind, Error, OutputProbe, SourceIdentity, SourceKind, SourceMetadata, SourceSnapshot,
	Timestamp,
};

#[path = "windows_directory.rs"]
mod directory;

pub(super) use directory::{DIRECTORY_BUFFER_BYTES, Directory};

const FILESYSTEM_NTFS: &str = "NTFS";
const VOLUME_GUID_PREFIX: &str = r"\\?\Volume{";
const VOLUME_GUID_END: char = '}';
const VOLUME_ROOT_SUFFIX: &str = "\\";
const MAX_PATH_UTF16: usize = 32_767;
const ADS_SEPARATOR: u16 = b':' as u16;
const PATH_TERMINATOR: u16 = 0;
const TRAILING_DOT: u16 = b'.' as u16;
const TRAILING_SPACE: u16 = b' ' as u16;
const TICKS_PER_SECOND: i64 = 10_000_000;
const NANOSECONDS_PER_TICK: i64 = 100;
const SECONDS_TO_UNIX_EPOCH: i64 = 11_644_473_600;
const PARENT_ACCESS: u32 = FILE_READ_ATTRIBUTES | FILE_TRAVERSE;
const UNSUPPORTED_ENTRY_ATTRIBUTES: u32 = FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE;
const OPEN_PARENT: &str = "open NTFS directory without following reparse points";
const STAT_SOURCE: &str = "read source information without following reparse points";
const STAT_OUTPUT: &str = "read output information without following reparse points";
const OPEN_SOURCE: &str = "open regular source without following reparse points";
const READ_DIRECTORY: &str = "enumerate source directory handle";

pub(super) struct Anchor;

impl Anchor {
	pub(super) fn new() -> Result<Self, Error> {
		Ok(Self)
	}

	pub(super) fn locate<'a>(&self, path: &'a Path) -> Result<Leaf<'a>, Error> {
		resolve_parent(path).map_err(|error| annotated(OPEN_PARENT, path, error))
	}

	pub(super) fn snapshot(
		&self,
		directory: &File,
		name: &OsStr,
		path: PathBuf,
	) -> Result<SourceSnapshot, Error> {
		let file = open_relative(directory, name, FILE_READ_ATTRIBUTES)
			.map_err(|error| annotated(STAT_SOURCE, &path, error))?;
		let (attributes, metadata) =
			observe(&file).map_err(|error| annotated(STAT_SOURCE, &path, error))?;
		let kind = source_kind(attributes)
			.ok_or_else(|| Error::UnsupportedSource { path: path.clone() })?;
		Ok(SourceSnapshot {
			path,
			metadata,
			attributes,
			kind,
		})
	}

	pub(super) fn probe_output(&self, path: &Path) -> Result<OutputProbe, Error> {
		let leaf = match resolve_parent(path) {
			Ok(leaf) => leaf,
			Err(error) if error.kind() == io::ErrorKind::NotFound => {
				return Ok(OutputProbe::default());
			}
			Err(error) => return Err(annotated(OPEN_PARENT, path, error)),
		};
		let (_, parent) =
			observe(&leaf.directory).map_err(|error| annotated(STAT_OUTPUT, path, error))?;
		let output = match open_relative(&leaf.directory, leaf.name, FILE_READ_ATTRIBUTES) {
			Ok(file) => {
				let (attributes, metadata) =
					observe(&file).map_err(|error| annotated(STAT_OUTPUT, path, error))?;
				if source_kind(attributes) != Some(SourceKind::RegularFile) {
					return Err(Error::UnsupportedOutput { path: path.into() });
				}
				Some(metadata.identity)
			}
			Err(error) if error.kind() == io::ErrorKind::NotFound => None,
			Err(error) => return Err(annotated(STAT_OUTPUT, path, error)),
		};
		Ok(OutputProbe {
			parent: Some(parent.identity),
			output,
		})
	}
}

pub(super) struct Leaf<'a> {
	pub(super) directory: File,
	pub(super) name: &'a OsStr,
}

pub(super) fn open_directory(
	parent: &File,
	name: &OsStr,
	source: &SourceSnapshot,
) -> Result<Directory, Error> {
	let file = open_relative(parent, name, PARENT_ACCESS | FILE_LIST_DIRECTORY)
		.map_err(|error| recheck_error(source, OPEN_PARENT, error))?;
	match_opened(source, &file)?;
	Ok(Directory::new(file))
}

pub(super) fn directory_handle<'a>(
	directory: &'a Directory,
	_source: &SourceSnapshot,
) -> Result<&'a File, Error> {
	Ok(directory.file())
}

pub(super) fn next_name(
	directory: &mut Directory,
	source: &SourceSnapshot,
) -> Result<Option<std::ffi::OsString>, Error> {
	directory
		.next_name()
		.map_err(|error| recheck_error(source, READ_DIRECTORY, error))
}

pub(super) const fn entry_kind(
	_parent: &File,
	_name: &OsStr,
	source: &SourceSnapshot,
) -> Result<EntryKind, Error> {
	match source.kind {
		SourceKind::Directory => Ok(EntryKind::Directory),
		SourceKind::RegularFile => Ok(EntryKind::File),
	}
}

pub(super) fn open_regular(source: &SourceSnapshot) -> Result<File, Error> {
	if source.kind != SourceKind::RegularFile {
		return Err(Error::RegularSourceRequired {
			path: source.path.clone(),
		});
	}
	let leaf =
		resolve_parent(&source.path).map_err(|error| recheck_error(source, OPEN_PARENT, error))?;
	let file = open_relative(&leaf.directory, leaf.name, FILE_GENERIC_READ)
		.map_err(|error| recheck_error(source, OPEN_SOURCE, error))?;
	match_opened(source, &file)?;
	Ok(file)
}

pub(super) fn validate(source: &SourceSnapshot) -> Result<(), Error> {
	let leaf =
		resolve_parent(&source.path).map_err(|error| recheck_error(source, OPEN_PARENT, error))?;
	let file = open_relative(&leaf.directory, leaf.name, FILE_READ_ATTRIBUTES)
		.map_err(|error| recheck_error(source, STAT_SOURCE, error))?;
	match_opened(source, &file)
}

pub(super) fn match_opened(source: &SourceSnapshot, file: &File) -> Result<(), Error> {
	let (attributes, mut metadata) =
		observe(file).map_err(|error| recheck_error(source, STAT_SOURCE, error))?;
	metadata.accessed = source.metadata.accessed;
	if source_kind(attributes) != Some(source.kind)
		|| metadata != source.metadata
		|| attributes != source.attributes
	{
		return Err(Error::SourceChanged {
			path: source.path.clone(),
		});
	}
	Ok(())
}

fn resolve_parent(path: &Path) -> io::Result<Leaf<'_>> {
	let invalid_path = || {
		io::Error::new(
			io::ErrorKind::InvalidInput,
			"expected an absolute drive path with no alternate data stream",
		)
	};
	if path
		.as_os_str()
		.encode_wide()
		.take(MAX_PATH_UTF16 + 1)
		.count()
		> MAX_PATH_UTF16
	{
		return Err(invalid_path());
	}
	let mut components = path.components();
	let Some(Component::Prefix(prefix)) = components.next() else {
		return Err(invalid_path());
	};
	let (drive, verbatim) = match prefix.kind() {
		Prefix::Disk(letter) => (letter, false),
		Prefix::VerbatimDisk(letter) => (letter, true),
		_ => return Err(invalid_path()),
	};
	if components.next() != Some(Component::RootDir) {
		return Err(invalid_path());
	}
	let mut names = Vec::new();
	for component in components {
		match component {
			Component::Normal(name) => {
				validate_name(name)?;
				names.push(name);
			}
			Component::ParentDir if !verbatim => {
				names.pop();
			}
			Component::CurDir if !verbatim => {}
			_ => return Err(invalid_path()),
		}
	}
	let name = names.pop().ok_or_else(invalid_path)?;
	let root = format!("{}:\\", char::from(drive));
	let mut directory = OpenOptions::new()
		.access_mode(PARENT_ACCESS)
		.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
		.open(root)?;
	require_directory(&directory)?;
	// A filesystem name can also describe a mapped share. Require the open
	// root to be a real mounted volume rather than a network or SUBST path.
	let final_root = QueryFinalPath::new(directory.try_clone()?)
		.with_flags(FinalPathFlags::VOLUME_NAME_GUID)
		.perform()
		.map_err(io::Error::other)?
		.to_string_lossy();
	let root_is_volume = final_root
		.strip_prefix(VOLUME_GUID_PREFIX)
		.and_then(|part| part.split_once(VOLUME_GUID_END))
		.is_some_and(|(identifier, suffix)| !identifier.is_empty() && suffix == VOLUME_ROOT_SUFFIX);
	if !root_is_volume {
		return Err(io::Error::new(
			io::ErrorKind::Unsupported,
			"Windows source discovery requires a local volume drive root",
		));
	}
	let volume = QueryVolumeInformation::new(directory.try_clone()?).perform()?;
	if volume.filesystem_name().to_string_lossy() != FILESYSTEM_NTFS {
		return Err(io::Error::new(
			io::ErrorKind::Unsupported,
			"Windows source discovery requires NTFS",
		));
	}
	for component in names {
		directory = open_relative(&directory, component, PARENT_ACCESS)?;
		require_directory(&directory)?;
	}
	Ok(Leaf { directory, name })
}

fn validate_name(name: &OsStr) -> io::Result<()> {
	let mut components = Path::new(name).components();
	if components.next() != Some(Component::Normal(name))
		|| components.next().is_some()
		|| name
			.encode_wide()
			.any(|unit| matches!(unit, ADS_SEPARATOR | PATH_TERMINATOR))
		|| matches!(
			name.encode_wide().last(),
			Some(TRAILING_DOT | TRAILING_SPACE)
		) {
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"expected one ordinary Windows path component",
		));
	}
	Ok(())
}

fn open_relative(parent: &File, name: &OsStr, access: u32) -> io::Result<File> {
	validate_name(name)?;
	fs_at::OpenOptions::default()
		.desired_access(access)
		.open_path_at(parent, name)
}

fn require_directory(file: &File) -> io::Result<()> {
	let information = QueryFileBasicInfo::new(file.try_clone()?).perform()?;
	if source_kind(information.file_attributes()) != Some(SourceKind::Directory) {
		return Err(io::Error::new(
			io::ErrorKind::NotADirectory,
			"a directory component is a reparse point or is not a directory",
		));
	}
	Ok(())
}

fn observe(file: &File) -> io::Result<(u32, SourceMetadata)> {
	let basic = QueryFileBasicInfo::new(file.try_clone()?).perform()?;
	let information = QueryFileInformationByHandle::new(file.try_clone()?).perform()?;
	let identity = SourceIdentity {
		device: u64::from(information.dwVolumeSerialNumber),
		inode: join_words(information.nFileIndexHigh, information.nFileIndexLow),
	};
	if identity.inode == 0
		|| (basic.file_attributes() & FILE_ATTRIBUTE_DIRECTORY == 0
			&& information.nNumberOfLinks == 0)
		|| basic.file_attributes() != information.dwFileAttributes
	{
		return Err(io::Error::new(
			io::ErrorKind::InvalidData,
			"NTFS source handle has incomplete or inconsistent identity",
		));
	}
	Ok((
		basic.file_attributes(),
		SourceMetadata {
			identity,
			size: join_words(information.nFileSizeHigh, information.nFileSizeLow),
			permissions: None,
			ownership: None,
			link_count: u64::from(information.nNumberOfLinks),
			accessed: timestamp(basic.last_access_time()),
			modified: timestamp(basic.last_write_time()),
			changed: timestamp(basic.change_time()),
			created: timestamp(basic.creation_time()),
			flags: None,
		},
	))
}

fn source_kind(attributes: u32) -> Option<SourceKind> {
	if attributes & UNSUPPORTED_ENTRY_ATTRIBUTES != 0 {
		None
	} else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
		Some(SourceKind::Directory)
	} else {
		Some(SourceKind::RegularFile)
	}
}

fn timestamp(ticks: i64) -> Option<Timestamp> {
	(ticks != 0).then(|| Timestamp {
		seconds: ticks.div_euclid(TICKS_PER_SECOND) - SECONDS_TO_UNIX_EPOCH,
		nanoseconds: (ticks.rem_euclid(TICKS_PER_SECOND) * NANOSECONDS_PER_TICK) as u32,
	})
}

fn join_words(high: u32, low: u32) -> u64 {
	(u64::from(high) << u32::BITS) | u64::from(low)
}

fn annotated(operation: &'static str, path: &Path, source: io::Error) -> Error {
	Error::Io {
		operation,
		path: path.into(),
		source,
	}
}

fn recheck_error(snapshot: &SourceSnapshot, operation: &'static str, source: io::Error) -> Error {
	match source.kind() {
		io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Error::SourceChanged {
			path: snapshot.path.clone(),
		},
		_ => annotated(operation, &snapshot.path, source),
	}
}
