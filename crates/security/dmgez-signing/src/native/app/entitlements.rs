use std::{
	collections::{BTreeMap, BTreeSet},
	ffi::OsStr,
	fs::{self, File, Metadata},
	io::{self, Read, Write},
	os::unix::fs::MetadataExt,
	path::{Component, Path, PathBuf},
};

use plist::{Dictionary, Value};
use rustix::fs::{Mode, OFlags, open};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{AppEntitlements, native::runner};

use super::{ROOT_ITEM, plan::Item};

mod xml;

#[cfg(test)]
mod tests;

const FILE_PREFIX: &str = ".entitlements-";
const FILE_SUFFIX: &str = ".plist";
const MAX_PLIST_BYTES: usize = 128 * 1024;
const MAX_TOTAL_PLIST_BYTES: usize = 8 * 1024 * 1024;
const GET_TASK_ALLOW: &str = "com.apple.security.get-task-allow";
const DISPLAY_ARGS: &[&str] = &["--display", "--entitlements", "-", "--xml"];
const ARCHITECTURE_OPTION: &str = "--architecture";
const END_OPTIONS: &str = "--";

#[derive(Debug, Error)]
pub(super) enum Error {
	#[error("entitlements item must be . or an exact bundle-relative path: {0:?}")]
	InvalidItem(PathBuf),
	#[error("entitlements rule did not match a signing item: {0:?}")]
	UnmatchedItem(PathBuf),
	#[error("multiple entitlements rules match signing item {0:?}")]
	DuplicateItem(PathBuf),
	#[error("could not open or read the entitlements file for {item:?}")]
	Read {
		item: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("entitlements input must be a stable bounded regular file for {0:?}")]
	InvalidFile(PathBuf),
	#[error("total entitlements input exceeds its byte limit")]
	TotalLimit,
	#[error("entitlements must be an unambiguous XML plist dictionary for {0:?}")]
	InvalidPlist(PathBuf),
	#[error(
		"entitlements for {0:?} require com.apple.security.get-task-allow to be false when supplied"
	)]
	DebuggingEnabled(PathBuf),
	#[error("could not stage the private entitlements file for {item:?}")]
	Stage {
		item: PathBuf,
		#[source]
		source: io::Error,
	},
	#[error("could not inspect native entitlements for {item:?}")]
	Inspect {
		item: PathBuf,
		#[source]
		source: runner::Error,
	},
	#[error("native entitlements differ from the signing rule for {0:?}")]
	Mismatch(PathBuf),
}

/// Holds independent, private copies of the exact policies used for signing.
pub(super) struct Prepared {
	rules: BTreeMap<PathBuf, Rule>,
}

struct Rule {
	file: NamedTempFile,
	dictionary: Dictionary,
}

pub(super) fn prepare(
	rules: &[AppEntitlements<'_>],
	items: &[Item],
	workdir: &Path,
) -> Result<Prepared, Error> {
	let items: BTreeSet<_> = items.iter().map(|item| item.relative.as_path()).collect();
	let mut prepared = BTreeMap::new();
	let mut remaining = MAX_TOTAL_PLIST_BYTES;
	for rule in rules {
		let relative = exact_item(rule.item)?;
		if !items.contains(relative.as_path()) {
			return Err(Error::UnmatchedItem(relative));
		}
		if prepared.contains_key(&relative) {
			return Err(Error::DuplicateItem(relative));
		}
		let bytes = read_plist(rule.plist, &relative, &mut remaining)?;
		let dictionary =
			parse_dictionary(&bytes).ok_or_else(|| Error::InvalidPlist(relative.clone()))?;
		if dictionary
			.get(GET_TASK_ALLOW)
			.is_some_and(|value| value.as_boolean() != Some(false))
		{
			return Err(Error::DebuggingEnabled(relative));
		}
		let file = stage_plist(&dictionary, workdir, &relative)?;
		prepared.insert(relative, Rule { file, dictionary });
	}
	Ok(Prepared { rules: prepared })
}

impl Prepared {
	pub(super) fn path(&self, relative: &Path) -> Option<&Path> {
		self.rules.get(relative).map(|rule| rule.file.path())
	}

	pub(super) fn verify(
		&self,
		relative: &Path,
		staged_item: &Path,
		architecture: Option<&str>,
	) -> Result<(), Error> {
		let mut args: Vec<_> = DISPLAY_ARGS.iter().map(OsStr::new).collect();
		if let Some(architecture) = architecture {
			args.extend([OsStr::new(ARCHITECTURE_OPTION), OsStr::new(architecture)]);
		}
		args.extend([OsStr::new(END_OPTIONS), staged_item.as_os_str()]);
		let output = runner::codesign(&args).map_err(|source| Error::Inspect {
			item: relative.to_path_buf(),
			source,
		})?;
		let expected = self.rules.get(relative).map(|rule| &rule.dictionary);
		if output.status.success() && matches_entitlements(expected, &output.stdout) {
			Ok(())
		} else {
			Err(Error::Mismatch(relative.to_path_buf()))
		}
	}
}

fn matches_entitlements(expected: Option<&Dictionary>, output: &[u8]) -> bool {
	if let Some(expected) = expected {
		parse_dictionary(output).as_ref() == Some(expected)
	} else {
		output.is_empty()
	}
}

fn exact_item(item: &Path) -> Result<PathBuf, Error> {
	if item.as_os_str() == ROOT_ITEM {
		return Ok(PathBuf::new());
	}
	let exact: PathBuf = item
		.components()
		.map(|part| match part {
			Component::Normal(name) => Ok(name),
			_ => Err(Error::InvalidItem(item.to_path_buf())),
		})
		.collect::<Result<_, _>>()?;
	if exact.as_os_str().is_empty() || exact.as_os_str() != item.as_os_str() {
		return Err(Error::InvalidItem(item.to_path_buf()));
	}
	Ok(exact)
}

fn read_plist(path: &Path, item: &Path, remaining: &mut usize) -> Result<Vec<u8>, Error> {
	let read_error = |source| Error::Read {
		item: item.to_path_buf(),
		source,
	};
	let initial = fs::symlink_metadata(path).map_err(read_error)?;
	let size =
		usize::try_from(initial.len()).map_err(|_| Error::InvalidFile(item.to_path_buf()))?;
	if !initial.is_file() || size > MAX_PLIST_BYTES {
		return Err(Error::InvalidFile(item.to_path_buf()));
	}
	*remaining = remaining.checked_sub(size).ok_or(Error::TotalLimit)?;
	let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
	let file =
		File::from(open(path, flags, Mode::empty()).map_err(|source| read_error(source.into()))?);
	if !same_file_version(&initial, &file.metadata().map_err(read_error)?) {
		return Err(Error::InvalidFile(item.to_path_buf()));
	}
	let mut bytes = Vec::with_capacity(size);
	(&file)
		.take(size as u64 + 1)
		.read_to_end(&mut bytes)
		.map_err(read_error)?;
	let current = file.metadata().map_err(read_error)?;
	if bytes.len() != size || !same_file_version(&initial, &current) {
		return Err(Error::InvalidFile(item.to_path_buf()));
	}
	Ok(bytes)
}

fn same_file_version(initial: &Metadata, current: &Metadata) -> bool {
	(initial.dev(), initial.ino()) == (current.dev(), current.ino())
		&& initial.mode() == current.mode()
		&& initial.len() == current.len()
		&& (initial.mtime(), initial.mtime_nsec()) == (current.mtime(), current.mtime_nsec())
		&& (initial.ctime(), initial.ctime_nsec()) == (current.ctime(), current.ctime_nsec())
}

fn parse_dictionary(bytes: &[u8]) -> Option<Dictionary> {
	let keys = xml::count_keys(bytes)?;
	let Value::Dictionary(dictionary) = Value::from_reader_xml(bytes).ok()? else {
		return None;
	};
	let stored_keys = dictionary.len() + dictionary.values().map(descendant_keys).sum::<usize>();
	// The plist reader stores dictionaries by key and would silently replace
	// repeated keys. Reject input if any keys were lost in that conversion.
	(stored_keys == keys).then_some(dictionary)
}

fn descendant_keys(value: &Value) -> usize {
	match value {
		Value::Dictionary(children) => {
			children.len() + children.values().map(descendant_keys).sum::<usize>()
		}
		Value::Array(children) => children.iter().map(descendant_keys).sum::<usize>(),
		_ => 0,
	}
}

fn stage_plist(
	dictionary: &Dictionary,
	workdir: &Path,
	item: &Path,
) -> Result<NamedTempFile, Error> {
	let stage_error = |source| Error::Stage {
		item: item.to_path_buf(),
		source,
	};
	let mut staged = tempfile::Builder::new()
		.prefix(FILE_PREFIX)
		.suffix(FILE_SUFFIX)
		.tempfile_in(workdir)
		.map_err(stage_error)?;
	let mut xml = Vec::new();
	plist::to_writer_xml(&mut xml, dictionary)
		.map_err(|_| Error::InvalidPlist(item.to_path_buf()))?;
	if xml.len() > MAX_PLIST_BYTES {
		return Err(Error::InvalidPlist(item.to_path_buf()));
	}
	staged.as_file_mut().write_all(&xml).map_err(stage_error)?;
	staged.as_file().sync_all().map_err(stage_error)?;
	Ok(staged)
}
