use std::{
	ffi::{OsStr, OsString},
	fs::{File, OpenOptions},
	io,
	os::windows::{
		ffi::{OsStrExt as _, OsStringExt as _},
		fs::OpenOptionsExt as _,
	},
	path::{Component, Path, Prefix},
};

use fs_at::os::windows::OpenOptionsExt as _;
use windows_namespace_request_sys::{
	FinalPathFlags, QueryFileInformationByHandle, QueryFinalPath, QueryVolumeInformation,
	file_basic::QueryFileBasicInfo,
	file_id::{FileIdInfo, QueryFileIdInfo},
	final_path::FinalPathError,
};
use windows_sys::Win32::Storage::FileSystem::{
	FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE,
	FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
	FILE_GENERIC_READ, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_TRAVERSE,
};

use super::ScanError;

mod directory;

use directory::{Directory, Entry};

const FILESYSTEM_NTFS: &str = "NTFS";
const VOLUME_GUID_PREFIX: &str = r"\\?\Volume{";
const VOLUME_GUID_END: char = '}';
const VOLUME_ROOT_SUFFIX: &str = "\\";
const SEPARATOR: u16 = b'\\' as u16;
const MAX_PATH_UTF16: usize = 32_767;
const MAX_NAME_UTF16: usize = 255;
const FIRST_PRINTABLE_UTF16: u16 = 32;
const TRAILING_DOT: u16 = b'.' as u16;
const TRAILING_SPACE: u16 = b' ' as u16;
const FORBIDDEN_COMPONENT: [u16; 9] = [
	b':' as u16,
	b'\\' as u16,
	b'/' as u16,
	b'*' as u16,
	b'?' as u16,
	b'"' as u16,
	b'<' as u16,
	b'>' as u16,
	b'|' as u16,
];
const PARENT_ACCESS: u32 = FILE_READ_ATTRIBUTES | FILE_TRAVERSE;
const DIRECTORY_ACCESS: u32 = PARENT_ACCESS | FILE_LIST_DIRECTORY;
const UNSUPPORTED_ATTRIBUTES: u32 =
	FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DEVICE | FILE_ATTRIBUTE_OFFLINE;
const NOT_REGULAR_FILE_ATTRIBUTES: u32 = UNSUPPORTED_ATTRIBUTES | FILE_ATTRIBUTE_DIRECTORY;

#[derive(Debug)]
pub(crate) struct LocalRoot(File);

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Stamp {
	id: FileIdInfo,
	size: u64,
	links: u32,
	created: i64,
	modified: i64,
	changed: i64,
	attributes: u32,
}

impl LocalRoot {
	pub(crate) fn open(root: &Path) -> io::Result<Self> {
		let absolute;
		let path = if root.is_absolute() {
			root
		} else {
			if root.as_os_str().is_empty()
				|| matches!(
					root.components().next(),
					Some(Component::Prefix(_) | Component::RootDir)
				) {
				return Err(invalid_root());
			}
			absolute = std::env::current_dir()?.join(root);
			&absolute
		};
		if path
			.as_os_str()
			.encode_wide()
			.take(MAX_PATH_UTF16 + 1)
			.count()
			> MAX_PATH_UTF16
		{
			return Err(invalid_root());
		}
		let mut components = path.components().peekable();
		let Some(Component::Prefix(prefix)) = components.next() else {
			return Err(invalid_root());
		};
		let drive = match prefix.kind() {
			Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
			_ => return Err(invalid_root()),
		};
		if components.next() != Some(Component::RootDir) {
			return Err(invalid_root());
		}
		let volume_path = format!("{}:{VOLUME_ROOT_SUFFIX}", char::from(drive));
		let mut directory = OpenOptions::new()
			.access_mode(if components.peek().is_none() {
				DIRECTORY_ACCESS
			} else {
				PARENT_ACCESS
			})
			.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
			.open(volume_path)?;
		require_directory(&directory)?;
		require_local_ntfs(&directory)?;
		while let Some(component) = components.next() {
			let Component::Normal(name) = component else {
				return Err(invalid_root());
			};
			let access = if components.peek().is_none() {
				DIRECTORY_ACCESS
			} else {
				PARENT_ACCESS
			};
			directory = open_relative(&directory, name, access)?;
			require_directory(&directory)?;
		}
		Ok(Self(directory))
	}

	pub(super) fn named_stamp(&self, name: &Path) -> io::Result<Stamp> {
		let file = open_relative(&self.0, name.as_os_str(), FILE_READ_ATTRIBUTES)?;
		let stamp = Stamp::capture(&file)?;
		if stamp.is_direct_file() {
			require_exact_name(&file, name.as_os_str())?;
		}
		Ok(stamp)
	}

	pub(super) fn open_data(&self, name: &Path) -> io::Result<File> {
		open_relative(&self.0, name.as_os_str(), FILE_GENERIC_READ)
	}

	pub(super) fn directory_stamp(&self) -> io::Result<Stamp> {
		Stamp::capture(&self.0)
	}

	pub(super) fn directory(&self) -> io::Result<Directory> {
		Ok(Directory::new(self.0.try_clone()?))
	}

	pub(super) fn is_direct_entry(&self, entry: &Entry) -> Result<bool, ScanError> {
		if entry.attributes & NOT_REGULAR_FILE_ATTRIBUTES != 0 {
			return Ok(false);
		}
		let file = open_relative(&self.0, entry.name(), FILE_READ_ATTRIBUTES).map_err(|error| {
			if error.kind() == io::ErrorKind::NotFound {
				ScanError::Changed
			} else {
				ScanError::Unavailable
			}
		})?;
		let stamp = Stamp::capture(&file)?;
		if stamp.id.file_id() != entry.id || stamp.attributes != entry.attributes {
			return Err(ScanError::Changed);
		}
		Ok(stamp.is_direct_file())
	}
}

impl Stamp {
	pub(super) fn capture(file: &File) -> io::Result<Self> {
		let basic = QueryFileBasicInfo::new(file.try_clone()?).perform()?;
		let information = QueryFileInformationByHandle::new(file.try_clone()?).perform()?;
		let id = QueryFileIdInfo::new(file.try_clone()?).perform()?;
		if id.file_id() == [0; 16] || basic.file_attributes() != information.dwFileAttributes {
			return Err(io::Error::new(
				io::ErrorKind::InvalidData,
				"NTFS handle has incomplete or inconsistent identity",
			));
		}
		Ok(Self {
			id,
			size: (u64::from(information.nFileSizeHigh) << u32::BITS)
				| u64::from(information.nFileSizeLow),
			links: information.nNumberOfLinks,
			created: basic.creation_time(),
			modified: basic.last_write_time(),
			changed: basic.change_time(),
			attributes: basic.file_attributes(),
		})
	}

	pub(super) const fn is_direct_file(&self) -> bool {
		self.attributes & NOT_REGULAR_FILE_ATTRIBUTES == 0 && self.links == 1
	}
}

fn invalid_root() -> io::Error {
	io::Error::new(
		io::ErrorKind::InvalidInput,
		"expected an ordinary local Windows drive path",
	)
}

pub(super) fn validate_component(name: &OsStr) -> io::Result<()> {
	let mut components = Path::new(name).components();
	let units = name.encode_wide();
	if components.next() != Some(Component::Normal(name))
		|| components.next().is_some()
		|| units.clone().take(MAX_NAME_UTF16 + 1).count() > MAX_NAME_UTF16
		|| units
			.clone()
			.any(|unit| unit < FIRST_PRINTABLE_UTF16 || FORBIDDEN_COMPONENT.contains(&unit))
		|| matches!(units.last(), Some(TRAILING_DOT | TRAILING_SPACE))
	{
		return Err(io::Error::new(
			io::ErrorKind::InvalidInput,
			"expected one ordinary Windows path component",
		));
	}
	Ok(())
}

fn open_relative(directory: &File, name: &OsStr, access: u32) -> io::Result<File> {
	validate_component(name)?;
	// fs_at uses the directory as the NT root and opens the final reparse point itself.
	fs_at::OpenOptions::default()
		.desired_access(access)
		.open_path_at(directory, name)
}

fn require_directory(file: &File) -> io::Result<()> {
	let basic = QueryFileBasicInfo::new(file.try_clone()?).perform()?;
	if basic.file_attributes() & (UNSUPPORTED_ATTRIBUTES | FILE_ATTRIBUTE_DIRECTORY)
		!= FILE_ATTRIBUTE_DIRECTORY
	{
		return Err(io::Error::new(
			io::ErrorKind::NotADirectory,
			"Windows directory component must be an ordinary directory",
		));
	}
	Ok(())
}

fn require_local_ntfs(directory: &File) -> io::Result<()> {
	let final_root = QueryFinalPath::new(directory.try_clone()?)
		.with_flags(FinalPathFlags::VOLUME_NAME_GUID)
		.perform()
		.map_err(final_path_error)?
		.to_string_lossy();
	let volume_root = final_root
		.strip_prefix(VOLUME_GUID_PREFIX)
		.and_then(|path| path.split_once(VOLUME_GUID_END))
		.is_some_and(|(id, suffix)| !id.is_empty() && suffix == VOLUME_ROOT_SUFFIX);
	if !volume_root {
		return Err(io::Error::new(
			io::ErrorKind::Unsupported,
			"Windows confinement requires a physical local volume drive root",
		));
	}
	let volume = QueryVolumeInformation::new(directory.try_clone()?).perform()?;
	if volume.filesystem_name().to_string_lossy() != FILESYSTEM_NTFS {
		return Err(io::Error::new(
			io::ErrorKind::Unsupported,
			"Windows confinement requires NTFS",
		));
	}
	Ok(())
}

fn require_exact_name(file: &File, name: &OsStr) -> io::Result<()> {
	// Native opens can resolve aliases. Read the stored leaf spelling from the
	// held handle without using its reported path for another open.
	let path = QueryFinalPath::new(file.try_clone()?)
		.with_flags(FinalPathFlags::VOLUME_NAME_NONE)
		.perform()
		.map_err(final_path_error)?;
	let Some(leaf) = path.as_units().rsplit(|unit| *unit == SEPARATOR).next() else {
		return Err(io::ErrorKind::NotFound.into());
	};
	if OsString::from_wide(leaf) != name {
		return Err(io::ErrorKind::NotFound.into());
	}
	Ok(())
}

fn final_path_error(error: FinalPathError) -> io::Error {
	match error {
		FinalPathError::Win32(error) => error.into(),
		error => io::Error::other(error),
	}
}
