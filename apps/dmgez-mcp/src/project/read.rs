use std::fmt::Write as _;

use dmgez_config::{Filesystem, ImageFormat};
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::local::LocalRoot;

use super::Issue;

pub(super) const SHA256_HEX_BYTES: usize = 64;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Reading {
	Read {
		revision_sha256: String,
		manifest_bytes: usize,
		content_entries: usize,
		image_format: ImageFormatName,
		filesystem: FilesystemName,
		reproducible: bool,
	},
	Unavailable {
		issue: Issue,
	},
	Busy,
}

impl LocalRoot {
	pub(crate) fn read(&self, file_name: &str) -> Reading {
		match self.parse_manifest(file_name) {
			Ok((project, input)) => Reading::Read {
				revision_sha256: revision_sha256(input.as_bytes()),
				manifest_bytes: input.len(),
				content_entries: project.contents.len(),
				image_format: project.image.format.into(),
				filesystem: project.image.filesystem.into(),
				reproducible: project.build.reproducible,
			},
			Err(issue) => Reading::Unavailable { issue },
		}
	}
}

pub(super) fn revision_sha256(bytes: &[u8]) -> String {
	let mut result = String::with_capacity(SHA256_HEX_BYTES);
	for byte in Sha256::digest(bytes) {
		write!(result, "{byte:02x}").expect("writing to a string cannot fail");
	}
	result
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImageFormatName {
	Udro,
	Udzo,
	Ulfo,
	Ulmo,
	Raw,
	Udrw,
	Udsb,
	Sparseimage,
	Asif,
	Udbz,
	Udco,
	Unknown,
}

impl From<ImageFormat> for ImageFormatName {
	fn from(format: ImageFormat) -> Self {
		match format {
			ImageFormat::Udro => Self::Udro,
			ImageFormat::Udzo => Self::Udzo,
			ImageFormat::Ulfo => Self::Ulfo,
			ImageFormat::Ulmo => Self::Ulmo,
			ImageFormat::Raw => Self::Raw,
			ImageFormat::Udrw => Self::Udrw,
			ImageFormat::Udsb => Self::Udsb,
			ImageFormat::Sparseimage => Self::Sparseimage,
			ImageFormat::Asif => Self::Asif,
			ImageFormat::Udbz => Self::Udbz,
			ImageFormat::Udco => Self::Udco,
			_ => Self::Unknown,
		}
	}
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FilesystemName {
	Hfsplus,
	Hfsx,
	Apfs,
	Unknown,
}

impl From<Filesystem> for FilesystemName {
	fn from(filesystem: Filesystem) -> Self {
		match filesystem {
			Filesystem::Hfsplus => Self::Hfsplus,
			Filesystem::Hfsx => Self::Hfsx,
			Filesystem::Apfs => Self::Apfs,
			_ => Self::Unknown,
		}
	}
}
