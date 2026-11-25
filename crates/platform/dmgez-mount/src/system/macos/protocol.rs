use std::{path::Path, process::Command, time::Duration};

use dmgez_core::process::{self, Output};
use serde::Deserialize;

use super::{
	super::{
		Error,
		record::{Device, Identity},
	},
	files::image_alias,
};

const HDIUTIL: &str = "/usr/bin/hdiutil";
const INFO: &str = "info";
const ATTACH: &str = "attach";
const DETACH: &str = "detach";
const READ_ONLY: &str = "-readonly";
const NO_BROWSE: &str = "-nobrowse";
const NO_AUTO_OPEN: &str = "-noautoopen";
const NO_VERIFY: &str = "-noverify";
const NO_AUTO_FSCK: &str = "-noautofsck";
const MOUNT: &str = "-mount";
const MOUNT_REQUIRED: &str = "required";
const MOUNTPOINT: &str = "-mountpoint";
const PLIST: &str = "-plist";
const DETACH_BUSY: &[u8] = b"hdiutil: detach failed - Resource busy";

const INVENTORY_TIMEOUT: Duration = Duration::from_secs(10);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(90);
const DETACH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
pub(super) struct Inventory {
	images: Vec<Image>,
}

#[derive(Debug, Deserialize)]
struct Image {
	#[serde(rename = "image-path")]
	path: String,
	#[serde(flatten)]
	devices: Devices,
}

#[derive(Debug, Deserialize)]
struct Devices {
	#[serde(rename = "system-entities")]
	entities: Vec<Entity>,
}

#[derive(Debug, Deserialize)]
struct Entity {
	#[serde(rename = "dev-entry")]
	device: String,
	#[serde(rename = "mount-point")]
	mountpoint: Option<String>,
}

impl Entity {
	fn has_mountpoint(&self, mountpoint: &Path) -> bool {
		self.mountpoint
			.as_deref()
			.map(Path::new)
			.is_some_and(|value| value.as_os_str() == mountpoint.as_os_str())
	}

	fn has_device(&self, device: &Device) -> bool {
		self.device == device.as_str()
			|| self
				.device
				.strip_prefix(device.as_str())
				.and_then(|suffix| suffix.strip_prefix('s'))
				.is_some_and(|slice| {
					!slice.is_empty() && slice.bytes().all(|byte| byte.is_ascii_digit())
				})
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Relation {
	Absent,
	Exact,
	Conflicting,
}

impl Inventory {
	pub(super) fn contains_selection(&self, image: &Path, id: Identity, mountpoint: &Path) -> bool {
		self.images.iter().any(|record| {
			image_related(record, image, id)
				|| record
					.devices
					.entities
					.iter()
					.any(|entity| entity.has_mountpoint(mountpoint))
		})
	}

	pub(super) fn contains_device(&self, device: &Device) -> bool {
		self.images.iter().any(|record| {
			record
				.devices
				.entities
				.iter()
				.any(|entity| entity.has_device(device))
		})
	}

	pub(super) fn relation(
		&self,
		image: &Path,
		id: Identity,
		mountpoint: &Path,
		device: &Device,
	) -> Relation {
		let mut exact = false;
		for record in &self.images {
			let related =
				image_related(record, image, id)
					|| record.devices.entities.iter().any(|entity| {
						entity.has_device(device) || entity.has_mountpoint(mountpoint)
					});
			if !related {
				continue;
			}
			let [entity] = record.devices.entities.as_slice() else {
				return Relation::Conflicting;
			};
			if exact
				|| Path::new(&record.path).as_os_str() != image.as_os_str()
				|| entity.device != device.as_str()
				|| !entity.has_mountpoint(mountpoint)
			{
				return Relation::Conflicting;
			}
			exact = true;
		}
		if exact {
			Relation::Exact
		} else {
			Relation::Absent
		}
	}
}

fn image_related(record: &Image, image: &Path, id: Identity) -> bool {
	let path = Path::new(&record.path);
	path.as_os_str() == image.as_os_str() || (path.is_absolute() && image_alias(path, id))
}

pub(super) fn inventory() -> Result<Inventory, Error> {
	let output = process::run(
		Command::new(HDIUTIL).args([INFO, PLIST]),
		INVENTORY_TIMEOUT,
		MAX_OUTPUT_BYTES,
	)?;
	if !output.status.success() {
		return Err(Error::InventoryRefused {
			status: output.status,
		});
	}
	parse_inventory(&output.stdout)
}

fn parse_inventory(xml: &[u8]) -> Result<Inventory, Error> {
	plist::from_bytes(xml).map_err(|_| Error::InvalidEvidence)
}

pub(super) fn attach(image: &Path, mountpoint: &Path) -> Result<Output, process::Error> {
	process::run(
		Command::new(HDIUTIL)
			.args([
				ATTACH,
				READ_ONLY,
				NO_BROWSE,
				NO_AUTO_OPEN,
				NO_VERIFY,
				NO_AUTO_FSCK,
				MOUNT,
				MOUNT_REQUIRED,
				MOUNTPOINT,
			])
			.arg(mountpoint)
			.arg(PLIST)
			.arg(image),
		ATTACH_TIMEOUT,
		MAX_OUTPUT_BYTES,
	)
}

pub(super) fn attached_device(xml: &[u8], mountpoint: &Path) -> Result<Device, Error> {
	let parsed: Devices = plist::from_bytes(xml).map_err(|_| Error::InvalidEvidence)?;
	let [entity] = parsed.entities.as_slice() else {
		return Err(Error::InvalidEvidence);
	};
	if !entity.has_mountpoint(mountpoint) {
		return Err(Error::InvalidEvidence);
	}
	Device::try_from(entity.device.clone()).map_err(|_| Error::InvalidEvidence)
}

pub(super) fn detach(device: &Device) -> Result<Output, process::Error> {
	process::run(
		Command::new(HDIUTIL).args([DETACH, device.as_str()]),
		DETACH_TIMEOUT,
		MAX_OUTPUT_BYTES,
	)
}

pub(super) fn detach_busy(output: &Output) -> bool {
	output
		.stderr
		.split(|&byte| byte == b'\n')
		.any(|line| line == DETACH_BUSY)
}

#[cfg(test)]
mod tests;
