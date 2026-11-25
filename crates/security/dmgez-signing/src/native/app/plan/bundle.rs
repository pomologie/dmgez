use std::{
	collections::BTreeSet,
	ffi::OsStr,
	fs::File,
	io::{Cursor, Read},
	os::unix::fs::MetadataExt,
	path::Path,
};

use plist::stream::{Event, Reader};
use rustix::fs::{self, Mode, OFlags};

use super::{
	Error, Item, Kind, classify, io_error, metadata, optional_metadata, require_single_link,
};

const CONTENTS: &str = "Contents";
const INFO: &str = "Info.plist";
const EXECUTABLES: &str = "MacOS";
const RESOURCES: &str = "Resources";
const EXECUTABLE_KEY: &str = "CFBundleExecutable";
const APP: &str = "app";
const BUNDLE_EXTENSIONS: &[&str] = &[
	APP,
	"appex",
	"systemextension",
	"dext",
	"xpc",
	"plugin",
	"bundle",
	"service",
	"action",
	"mdimporter",
	"qlgenerator",
];
const CODE_LOCATIONS: &[&str] = &[
	"Contents/Frameworks",
	"Contents/Helpers",
	"Contents/Library/Automator",
	"Contents/Library/LaunchServices",
	"Contents/Library/LoginItems",
	"Contents/Library/Spotlight",
	"Contents/Library/SystemExtensions",
	"Contents/MacOS",
	"Contents/PlugIns",
	"Contents/XPCServices",
];
const MAX_PLIST_BYTES: u64 = 1 << 20;
const MAX_PLIST_EVENTS: usize = 16_384;
const MAX_PLIST_DEPTH: usize = 32;
const MAX_PLIST_EXPANSION: usize = 16 << 20;
pub(super) const EXECUTE_BITS: u32 = 0o111;
const SCRIPT_MAGIC: &[u8; 2] = b"#!";

pub(super) fn is_bundle_extension(extension: &OsStr) -> bool {
	BUNDLE_EXTENSIONS
		.iter()
		.any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

pub(super) fn ordinary(
	root: &Path,
	relative: &Path,
	remaining: &mut u64,
) -> Result<Option<Item>, Error> {
	let contents = relative.join(CONTENTS);
	let path = root.join(&contents);
	let application = relative.as_os_str().is_empty()
		|| relative
			.extension()
			.is_some_and(|extension| extension.eq_ignore_ascii_case(APP));
	let code_location = is_code_location(relative);
	match optional_metadata(&path)? {
		Some(metadata) if metadata.is_dir() => {}
		None if !application && !code_location => return Ok(None),
		_ => return Err(Error::Bundle(path)),
	}
	let info = contents.join(INFO);
	if !application && !code_location && optional_metadata(&root.join(&info))?.is_none() {
		return Ok(None);
	}
	match read_executable(&root.join(&info), remaining)? {
		Some(name) => {
			let directory = contents.join(EXECUTABLES);
			let path = root.join(&directory);
			if !metadata(&path)?.is_dir() {
				return Err(Error::Bundle(path));
			}
			code(root, relative, &directory, &name).map(Some)
		}
		None if application => Err(Error::Bundle(root.join(info))),
		None if code_location => Ok(Some(Item {
			relative: relative.to_path_buf(),
			executable: info,
			hardened: false,
		})),
		None => Ok(None),
	}
}

pub(super) fn framework(root: &Path, relative: &Path, remaining: &mut u64) -> Result<Item, Error> {
	let resources = root.join(relative).join(RESOURCES);
	if !metadata(&resources)?.is_dir() {
		return Err(Error::Bundle(resources));
	}
	let info = resources.join(INFO);
	let name = read_executable(&info, remaining)?.ok_or_else(|| Error::Bundle(info))?;
	code(root, relative, relative, &name)
}

fn code(root: &Path, relative: &Path, directory: &Path, name: &str) -> Result<Item, Error> {
	let executable = directory.join(name);
	let path = root.join(&executable);
	let metadata = metadata(&path)?;
	if !metadata.is_file() {
		return Err(Error::Bundle(path));
	}
	require_single_link(&path, &metadata)?;
	let kind = classify(&path)?;
	if (kind.is_none() && !is_script(&path)?)
		|| ((kind.is_none() || matches!(kind, Some(Kind::Executable)))
			&& metadata.mode() & EXECUTE_BITS == 0)
	{
		return Err(Error::Bundle(path));
	}
	Ok(Item {
		relative: relative.to_path_buf(),
		executable,
		hardened: matches!(kind, Some(Kind::Executable)),
	})
}

fn read_executable(path: &Path, remaining: &mut u64) -> Result<Option<String>, Error> {
	let file = open_regular(path)?;
	let mut bytes = Vec::new();
	file.take(MAX_PLIST_BYTES.min(*remaining) + 1)
		.read_to_end(&mut bytes)
		.map_err(|source| io_error(path, source))?;
	if bytes.len() as u64 > MAX_PLIST_BYTES {
		return Err(Error::Limit("Info.plist bytes"));
	}
	*remaining = remaining
		.checked_sub(bytes.len() as u64)
		.ok_or(Error::Limit("total Info.plist bytes"))?;
	extract_executable(&bytes).ok_or_else(|| Error::Bundle(path.to_path_buf()))
}

fn extract_executable(bytes: &[u8]) -> Option<Option<String>> {
	let mut events = Reader::new(Cursor::new(bytes));
	let root = events.next()?.ok()?;
	if !matches!(root, Event::StartDictionary(_)) {
		return None;
	}
	let mut executable = None;
	let mut keys = BTreeSet::new();
	let mut pending_key = None;
	let mut depth: usize = 1;
	let mut remaining_bytes = MAX_PLIST_EXPANSION.checked_sub(expanded_bytes(&root))?;
	for event in events.by_ref().take(MAX_PLIST_EVENTS) {
		let event = event.ok()?;
		remaining_bytes = remaining_bytes.checked_sub(expanded_bytes(&event))?;
		if matches!(event, Event::EndCollection) {
			depth = depth.checked_sub(1)?;
			if depth == 0 {
				if pending_key.is_some() || events.next().is_some() {
					return None;
				}
				return Some(executable);
			}
			continue;
		}
		if depth == 1 {
			if let Some(key_is_executable) = pending_key.take() {
				if key_is_executable {
					let Event::String(name) = event else {
						return None;
					};
					if !is_filename(&name) {
						return None;
					}
					executable = Some(name.into_owned());
					continue;
				}
			} else {
				let Event::String(key) = event else {
					return None;
				};
				pending_key = Some(key == EXECUTABLE_KEY);
				if !keys.insert(key) {
					return None;
				}
				continue;
			}
		}
		if matches!(event, Event::StartDictionary(_) | Event::StartArray(_)) {
			depth += 1;
			if depth > MAX_PLIST_DEPTH {
				return None;
			}
		}
	}
	None
}

fn expanded_bytes(event: &Event<'_>) -> usize {
	match event {
		Event::String(value) => value.len(),
		Event::Data(value) => value.len(),
		Event::StartArray(Some(count)) => container_bytes(*count, 1),
		Event::StartDictionary(Some(count)) => container_bytes(*count, 2),
		_ => 0,
	}
}

fn container_bytes(count: u64, references: usize) -> usize {
	usize::try_from(count)
		.unwrap_or(usize::MAX)
		.saturating_mul(references * size_of::<u64>())
}

fn is_filename(name: &str) -> bool {
	!name.as_bytes().contains(&0) && Path::new(name).file_name() == Some(OsStr::new(name))
}

pub(super) fn is_code_location(relative: &Path) -> bool {
	relative.parent().is_some_and(|parent| {
		CODE_LOCATIONS
			.iter()
			.any(|location| parent.ends_with(location))
	})
}

pub(super) fn is_script(path: &Path) -> Result<bool, Error> {
	let mut magic = [0; SCRIPT_MAGIC.len()];
	match open_regular(path)?.read_exact(&mut magic) {
		Ok(()) => Ok(&magic == SCRIPT_MAGIC),
		Err(source) if source.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
		Err(source) => Err(io_error(path, source)),
	}
}

fn open_regular(path: &Path) -> Result<File, Error> {
	let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
	let file = fs::open(path, flags, Mode::empty())
		.map(File::from)
		.map_err(|source| io_error(path, source.into()))?;
	if !file
		.metadata()
		.map_err(|source| io_error(path, source))?
		.is_file()
	{
		return Err(Error::Bundle(path.to_path_buf()));
	}
	Ok(file)
}
