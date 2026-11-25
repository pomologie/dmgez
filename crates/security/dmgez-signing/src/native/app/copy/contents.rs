use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use rustix::fs;
use sha2::{Digest, Sha256};

use super::{CopyError, at};

const MAX_DATA_BYTES: u64 = 1 << 40;
const MAX_ATTRIBUTE_BYTES: usize = 128 << 20;
const MAX_ATTRIBUTE_VALUE_BYTES: usize = 16 << 20;
const MAX_ATTRIBUTE_NAMES_BYTES: usize = 64 << 10;
const HASH_BUFFER_BYTES: usize = 32 << 10;
const ATTRIBUTE_SEPARATOR: u8 = 0;
const QUARANTINE: &[u8] = b"com.apple.quarantine";

type Fingerprint = [u8; 32];

pub(super) struct Budget {
	data: u64,
	attributes: usize,
}

impl Budget {
	pub(super) fn new() -> Self {
		Self {
			data: MAX_DATA_BYTES,
			attributes: MAX_ATTRIBUTE_BYTES,
		}
	}

	fn data(&mut self, bytes: u64) -> Result<(), CopyError> {
		self.data = self
			.data
			.checked_sub(bytes)
			.ok_or(CopyError::Limit("file bytes"))?;
		Ok(())
	}

	fn attributes(&mut self, bytes: usize) -> Result<(), CopyError> {
		self.attributes = self
			.attributes
			.checked_sub(bytes)
			.ok_or(CopyError::Limit("attribute bytes"))?;
		Ok(())
	}
}

#[derive(Eq, PartialEq)]
pub(super) struct Contents {
	data: Option<Fingerprint>,
	attributes: BTreeMap<Vec<u8>, Attribute>,
}

#[derive(Eq, PartialEq)]
enum Attribute {
	Value(Fingerprint),
	// macOS copy semantics can update the quarantine record. Verify that
	// ditto retains the attribute, and leave the native record intact.
	Quarantine,
}

impl Contents {
	pub(super) fn read(
		file: &File,
		path: &Path,
		length: Option<i64>,
		budget: &mut Budget,
	) -> Result<Self, CopyError> {
		let data = length
			.map(|length| data(file, path, length, budget))
			.transpose()?;
		let attributes = attributes(file, path, budget)?;
		Ok(Self { data, attributes })
	}
}

fn data(
	mut file: &File,
	path: &Path,
	length: i64,
	budget: &mut Budget,
) -> Result<Fingerprint, CopyError> {
	let mut remaining = u64::try_from(length).map_err(|_| CopyError::Limit("file bytes"))?;
	budget.data(remaining)?;
	let mut buffer = [0_u8; HASH_BUFFER_BYTES];
	let mut hash = Sha256::new();
	while remaining != 0 {
		let len = remaining.min(HASH_BUFFER_BYTES as u64) as usize;
		file.read_exact(&mut buffer[..len])
			.map_err(|error| at(path, error))?;
		hash.update(&buffer[..len]);
		remaining -= len as u64;
	}
	Ok(hash.finalize().into())
}

fn attributes(
	file: &File,
	path: &Path,
	budget: &mut Budget,
) -> Result<BTreeMap<Vec<u8>, Attribute>, CopyError> {
	let bytes = fs::flistxattr_query(file).map_err(|error| at(path, error))?;
	if bytes > MAX_ATTRIBUTE_NAMES_BYTES {
		return Err(CopyError::Limit("attribute names"));
	}
	budget.attributes(bytes)?;
	let mut names = vec![0; bytes];
	if bytes == 0 {
		return Ok(BTreeMap::new());
	}
	let actual = fs::flistxattr(file, names.as_mut_slice()).map_err(|error| at(path, error))?;
	if actual != bytes {
		return Err(CopyError::SourceChanged(path.to_path_buf()));
	}
	let names = names
		.strip_suffix(&[ATTRIBUTE_SEPARATOR])
		.ok_or_else(|| CopyError::Unsupported(path.to_path_buf()))?;
	let mut result = BTreeMap::new();
	for name in names.split(|&byte| byte == ATTRIBUTE_SEPARATOR) {
		if name.is_empty() {
			return Err(CopyError::Unsupported(path.to_path_buf()));
		}
		let value = if name == QUARANTINE {
			Attribute::Quarantine
		} else {
			let bytes = fs::fgetxattr_query(file, name).map_err(|error| at(path, error))?;
			if bytes > MAX_ATTRIBUTE_VALUE_BYTES {
				return Err(CopyError::Limit("attribute value"));
			}
			budget.attributes(bytes)?;
			let mut value = vec![0; bytes];
			let actual =
				fs::fgetxattr(file, name, value.as_mut_slice()).map_err(|error| at(path, error))?;
			if actual != bytes {
				return Err(CopyError::SourceChanged(path.to_path_buf()));
			}
			Attribute::Value(Sha256::digest(&value).into())
		};
		if result.insert(name.to_vec(), value).is_some() {
			return Err(CopyError::Unsupported(path.to_path_buf()));
		}
	}
	Ok(result)
}
