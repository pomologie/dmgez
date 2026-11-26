use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_NATIVE_PATH_BYTES: usize = 1023;
const WHOLE_DISK_PREFIX: &str = "/dev/disk";

/// Evidence of one established native disk-image attachment.
///
/// A deserialized record must be revalidated against the host before use.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
	pub(super) image: CanonicalPath,
	pub(super) mountpoint: CanonicalPath,
	pub(super) device: Device,
	pub(super) source: FileStamp,
	pub(super) underlying: Identity,
	pub(super) mount: MountedIdentity,
	pub(super) node: DeviceStamp,
}

impl Attachment {
	/// The exact canonical image path supplied to the host.
	#[must_use]
	pub fn image(&self) -> &Path {
		self.image.as_path()
	}

	/// The exact canonical mountpoint supplied to the host.
	#[must_use]
	pub fn mountpoint(&self) -> &Path {
		self.mountpoint.as_path()
	}

	/// The native whole-disk path observed for the image.
	#[must_use]
	pub fn device(&self) -> &str {
		self.device.as_str()
	}
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub(super) struct CanonicalPath(#[serde(deserialize_with = "deserialize_path")] PathBuf);

impl CanonicalPath {
	pub(super) fn as_path(&self) -> &Path {
		&self.0
	}
}

impl TryFrom<PathBuf> for CanonicalPath {
	type Error = &'static str;

	fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
		if !valid_native_path(&path) {
			return Err("expected a normalized absolute UTF-8 path");
		}
		Ok(Self(path))
	}
}

pub(crate) fn valid_native_path(path: &Path) -> bool {
	path.is_absolute()
		&& path.file_name().is_some()
		&& path.to_str().is_some_and(|text| {
			text.len() <= MAX_NATIVE_PATH_BYTES && !text.as_bytes().contains(&0)
		})
		&& !path
			.components()
			.any(|part| matches!(part, Component::ParentDir))
		&& path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
}

fn deserialize_path<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> Result<PathBuf, D::Error> {
	let path = PathBuf::deserialize(deserializer)?;
	CanonicalPath::try_from(path)
		.map(|path| path.0)
		.map_err(serde::de::Error::custom)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(super) struct Device(#[serde(deserialize_with = "deserialize_device")] String);

impl Device {
	pub(super) fn as_str(&self) -> &str {
		&self.0
	}

	pub(super) fn parse(device: &str) -> Result<(), &'static str> {
		let Some(index) = device.strip_prefix(WHOLE_DISK_PREFIX) else {
			return Err("expected a native whole-disk device");
		};
		let number: u32 = index.parse().map_err(|_| "expected a whole-disk number")?;
		if number.to_string() != index {
			return Err("expected a canonical whole-disk number");
		}
		Ok(())
	}
}

impl TryFrom<String> for Device {
	type Error = &'static str;

	fn try_from(device: String) -> Result<Self, Self::Error> {
		Self::parse(&device)?;
		Ok(Self(device))
	}
}

fn deserialize_device<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> Result<String, D::Error> {
	let device = String::deserialize(deserializer)?;
	Device::try_from(device)
		.map(|device| device.0)
		.map_err(serde::de::Error::custom)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
	pub(super) device: i32,
	pub(super) inode: u64,
	pub(super) created: Timestamp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Timestamp {
	pub(super) seconds: i64,
	pub(super) nanos: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileStamp {
	pub(super) id: Identity,
	pub(super) length: i64,
	pub(super) modified: Timestamp,
	pub(super) changed: Timestamp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MountedIdentity {
	pub(super) root: Identity,
	pub(super) fsid: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceStamp {
	pub(super) id: Identity,
	pub(super) rdev: i32,
	pub(super) changed: Timestamp,
}
