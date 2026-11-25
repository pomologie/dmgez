//! Bounded POSIX metadata reads through the identity-checked source handle.

use std::{collections::TryReserveError, fs::File, mem::size_of};

use dmgez_core::source::{CapturedFile, SourceError};
use rustix::{fs, io::Errno};
use thiserror::Error;

use super::{Attribute, AttributeLimits, Attributes};
use crate::manifest::{self, SourceSnapshot};

const ATTRIBUTE_SEPARATOR: u8 = 0;
const FINDER_INFO_NAME: &[u8] = b"com.apple.FinderInfo";
const RESOURCE_FORK_NAME: &[u8] = b"com.apple.ResourceFork";

/// A source or its metadata could not be captured within the requested limits.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("source identity check failed")]
	Manifest(#[from] manifest::Error),
	#[error("source data fork could not be captured")]
	Data(#[from] SourceError),
	#[error("attribute count or size exceeds the admitted metadata budget")]
	Limit,
	#[error("could not allocate within the admitted attribute budget")]
	Allocation(#[from] TryReserveError),
	#[error("could not read extended attributes from the source handle")]
	Attributes(#[source] Errno),
	#[error("source attributes changed during capture")]
	Changed,
	#[error("host returned a malformed extended attribute name list")]
	Names,
	#[error("Finder information must contain exactly 32 bytes")]
	FinderInfo,
}

/// Captures the file's data fork and attributes through the same admitted
/// descriptor. Attribute names use host bytes and are returned in byte order.
/// A planner must separately decide how to represent each one in its target.
///
/// # Errors
/// Rejects changed inputs, unavailable metadata, malformed Finder information,
/// or an allocation exceeding the caller's bounds.
pub fn capture_regular(
	snapshot: &SourceSnapshot,
	limits: AttributeLimits,
) -> Result<(CapturedFile, Attributes), Error> {
	let file = snapshot.open_regular()?;
	let attributes = read_attributes(&file, limits)?;
	let captured = CapturedFile::new(file)?;
	snapshot.validate()?;
	Ok((captured, attributes))
}

fn read_attributes(file: &File, limits: AttributeLimits) -> Result<Attributes, Error> {
	let names_bytes = fs::flistxattr_query(file).map_err(Error::Attributes)?;
	let mut remaining_bytes = limits.bytes;
	reserve_budget(&mut remaining_bytes, names_bytes)?;
	let mut names = buffer(names_bytes)?;
	let actual = fs::flistxattr(file, names.as_mut_slice()).map_err(attribute_race)?;
	if actual != names_bytes {
		return Err(Error::Changed);
	}
	let mut names = names.as_slice();
	if !names.is_empty() {
		names = names
			.strip_suffix(&[ATTRIBUTE_SEPARATOR])
			.ok_or(Error::Names)?;
	}
	let mut result = Attributes {
		finder_info: None,
		resource_fork: None,
		extended: Vec::new(),
		accounted_bytes: names_bytes,
	};
	if names_bytes == 0 {
		return Ok(result);
	}
	let names = names.split(|&byte| byte == ATTRIBUTE_SEPARATOR);
	let count = names.clone().count();
	if count > limits.attributes {
		return Err(Error::Limit);
	}
	reserve_budget(
		&mut remaining_bytes,
		count
			.checked_mul(size_of::<Attribute>())
			.ok_or(Error::Limit)?,
	)?;
	result.extended.try_reserve_exact(count)?;
	for name in names {
		if name.is_empty() {
			return Err(Error::Names);
		}
		let size = fs::fgetxattr_query(file, name).map_err(attribute_race)?;
		if size > limits.value_bytes {
			return Err(Error::Limit);
		}
		reserve_budget(&mut remaining_bytes, size)?;
		let mut value = buffer(size)?;
		let actual = fs::fgetxattr(file, name, value.as_mut_slice()).map_err(attribute_race)?;
		if actual != size {
			return Err(Error::Changed);
		}
		if cfg!(target_os = "macos") && name == FINDER_INFO_NAME {
			if result.finder_info.is_some() {
				return Err(Error::Names);
			}
			result.finder_info = Some(value.try_into().map_err(|_| Error::FinderInfo)?);
		} else if cfg!(target_os = "macos") && name == RESOURCE_FORK_NAME {
			if result.resource_fork.replace(value).is_some() {
				return Err(Error::Names);
			}
		} else {
			reserve_budget(&mut remaining_bytes, name.len())?;
			let mut stored = Vec::new();
			stored.try_reserve_exact(name.len())?;
			stored.extend_from_slice(name);
			result.extended.push(Attribute {
				name: stored,
				value,
			});
		}
	}
	result
		.extended
		.sort_unstable_by(|left, right| left.name.cmp(&right.name));
	if result
		.extended
		.windows(2)
		.any(|pair| pair[0].name == pair[1].name)
	{
		return Err(Error::Names);
	}
	result.accounted_bytes = limits.bytes - remaining_bytes;
	Ok(result)
}

fn reserve_budget(remaining: &mut usize, bytes: usize) -> Result<(), Error> {
	*remaining = remaining.checked_sub(bytes).ok_or(Error::Limit)?;
	Ok(())
}

fn buffer(bytes: usize) -> Result<Vec<u8>, Error> {
	let mut buffer = Vec::new();
	buffer.try_reserve_exact(bytes)?;
	buffer.resize(bytes, 0);
	Ok(buffer)
}

fn attribute_race(error: Errno) -> Error {
	if error == Errno::RANGE {
		return Error::Changed;
	}
	Error::Attributes(error)
}

#[cfg(test)]
mod tests {
	use std::fs;

	use dmgez_config::Project;
	use rustix::fs::{XattrFlags, fsetxattr};

	use super::{AttributeLimits, Error, capture_regular};
	use crate::manifest::{Limits, discover};

	const CONTENT: &[u8] = b"regular payload";
	const ATTRIBUTE: &str = "user.pomologie.payload";
	const EMPTY_ATTRIBUTE: &str = "user.pomologie.empty";
	const ATTRIBUTE_VALUE: &[u8] = &[0, 0xff, 0, 1];
	const INPUT: &str = "source";
	const CONFIG: &str = "schema_version=1\n[image]\nname='Metadata'\noutput='image.dmg'\n[[contents]]\nsource='source'\n";
	const LIMITS: AttributeLimits = AttributeLimits {
		bytes: 4096,
		value_bytes: 256,
		attributes: 8,
	};

	#[test]
	fn captures_exact_data_and_ordered_bounded_attributes() {
		let root = tempfile::tempdir().unwrap();
		let root_path = root.path().canonicalize().unwrap();
		let path = root_path.join(INPUT);
		fs::write(&path, CONTENT).unwrap();
		let file = fs::File::open(&path).unwrap();
		fsetxattr(&file, ATTRIBUTE, ATTRIBUTE_VALUE, XattrFlags::empty()).unwrap();
		fsetxattr(&file, EMPTY_ATTRIBUTE, b"", XattrFlags::empty()).unwrap();
		let manifest = discover(
			&Project::parse(CONFIG).unwrap(),
			&root_path,
			Limits::default(),
		)
		.unwrap();
		let source = manifest.entries()[0].source.as_ref().unwrap();
		let (data, attributes) = capture_regular(source, LIMITS).unwrap();
		let mut bytes = [0; CONTENT.len()];
		data.read_exact_at(0, &mut bytes).unwrap();
		assert_eq!(&bytes, CONTENT);
		assert!(
			attributes
				.extended
				.windows(2)
				.all(|pair| pair[0].name < pair[1].name)
		);
		for (name, expected) in [
			(EMPTY_ATTRIBUTE, b"".as_slice()),
			(ATTRIBUTE, ATTRIBUTE_VALUE),
		] {
			let stored = attributes
				.extended
				.iter()
				.find(|attr| attr.name == name.as_bytes())
				.unwrap();
			assert_eq!(stored.value, expected);
		}
		assert!(attributes.finder_info.is_none());
		assert!(attributes.resource_fork.is_none());
		assert!(attributes.accounted_bytes >= ATTRIBUTE.len() + ATTRIBUTE_VALUE.len());

		assert!(matches!(
			capture_regular(
				source,
				AttributeLimits {
					value_bytes: 3,
					..LIMITS
				}
			),
			Err(Error::Limit)
		));
		assert!(matches!(
			capture_regular(
				source,
				AttributeLimits {
					attributes: 1,
					..LIMITS
				}
			),
			Err(Error::Limit)
		));
		fs::write(&path, b"changed length and content").unwrap();
		assert!(matches!(
			capture_regular(source, LIMITS),
			Err(Error::Manifest(_))
		));
	}

	#[cfg(target_os = "macos")]
	#[test]
	fn separates_native_finder_info_and_resource_fork() {
		use super::{FINDER_INFO_NAME, RESOURCE_FORK_NAME};
		use crate::metadata::FINDER_INFO_BYTES;

		let root = tempfile::tempdir().unwrap();
		let root_path = root.path().canonicalize().unwrap();
		let path = root_path.join(INPUT);
		fs::write(&path, CONTENT).unwrap();
		let file = fs::File::open(&path).unwrap();
		let mut finder = [0; FINDER_INFO_BYTES];
		finder[..b"TEXT".len()].copy_from_slice(b"TEXT");
		fsetxattr(&file, FINDER_INFO_NAME, &finder, XattrFlags::empty()).unwrap();
		fsetxattr(
			&file,
			RESOURCE_FORK_NAME,
			ATTRIBUTE_VALUE,
			XattrFlags::empty(),
		)
		.unwrap();
		let manifest = discover(
			&Project::parse(CONFIG).unwrap(),
			&root_path,
			Limits::default(),
		)
		.unwrap();
		let source = manifest.entries()[0].source.as_ref().unwrap();
		let (_, attributes) = capture_regular(source, LIMITS).unwrap();
		assert_eq!(attributes.finder_info, Some(finder));
		assert_eq!(attributes.resource_fork.as_deref(), Some(ATTRIBUTE_VALUE));
		assert!(
			attributes
				.extended
				.iter()
				.all(|attr| attr.name != FINDER_INFO_NAME && attr.name != RESOURCE_FORK_NAME)
		);
	}
}
