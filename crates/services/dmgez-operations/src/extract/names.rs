use dmgez_vfs::volume::RawName;
use thiserror::Error;

use super::{NameBudget, NamePlanError, charge};

#[cfg(kani)]
#[path = "../../verification/kani/extract_names.rs"]
mod extract_name_proofs;

const CURRENT_DIRECTORY: &[u8] = b".";
const PARENT_DIRECTORY: &[u8] = b"..";
const POSIX_SEPARATOR: u8 = b'/';
const NUL: u8 = b'\0';
const WIN32_FORBIDDEN: &[u8] = b"<>:\"/\\|?*";
const WIN32_LITERAL_DEVICES: [&[u8]; 6] = [b"CON", b"PRN", b"AUX", b"NUL", b"CONIN$", b"CONOUT$"];
const NUMBERED_DEVICE_PREFIXES: [&[u8]; 2] = [b"COM", b"LPT"];
const EXTENSION_SEPARATOR: u8 = b'.';
const ASCII_SPACE: u8 = b' ';
const PORTABLE_NAME_SUFFIXES: [u8; 2] = [ASCII_SPACE, EXTENSION_SEPARATOR];

/// The destination comparison semantics qualified by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NameProfile {
	/// Printable ASCII with conservative Win32 exclusions and ASCII case folding.
	PortableAscii,
	/// Exact UTF-8 bytes on a POSIX directory with bytewise name comparison.
	PosixUtf8,
}

/// A single relative name component admitted under the plan's name profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostName(String);

impl HostName {
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub(super) fn admit(
		name: RawName,
		profile: NameProfile,
		max_bytes: usize,
	) -> Result<Self, NameError> {
		let name = match name {
			RawName::Utf8(name) => name,
			RawName::Utf16(units) if units.len() > max_bytes => return Err(NameError::TooLong),
			RawName::Utf16(units) => String::from_utf16(&units).map_err(|_| NameError::Encoding)?,
			_ => return Err(NameError::Encoding),
		};
		validate_component(name.as_bytes(), profile, max_bytes)?;
		Ok(Self(name))
	}

	pub(super) fn comparison_key(&self, profile: NameProfile) -> String {
		match profile {
			NameProfile::PortableAscii => self.0.to_ascii_lowercase(),
			NameProfile::PosixUtf8 => self.0.clone(),
		}
	}
}

// Charge a separator only when the child follows another path component.
pub(super) fn charge_child_path<E: std::error::Error + 'static>(
	parent_bytes: usize,
	has_parent: bool,
	name_bytes: usize,
	limit: usize,
) -> Result<usize, NamePlanError<E>> {
	let budget = NameBudget::RelativePathBytes;
	let prefix = charge(parent_bytes, usize::from(has_parent), limit, budget)?;
	charge(prefix, name_bytes, limit, budget)
}

/// The stored component cannot be preserved by the selected destination profile.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum NameError {
	#[error("entry name is not a single relative component")]
	NotComponent,
	#[error("entry name has no lossless encoding in the destination name profile")]
	Encoding,
	#[error("entry name exceeds the destination component byte limit")]
	TooLong,
	#[error("entry name contains a character the portable profile forbids")]
	ForbiddenCharacter,
	#[error("entry name starts or ends with a character the portable profile trims")]
	TrimmedName,
	#[error("entry name resolves to a reserved Win32 device")]
	DeviceName,
}

fn validate_component(
	name: &[u8],
	profile: NameProfile,
	max_bytes: usize,
) -> Result<(), NameError> {
	if name.len() > max_bytes {
		return Err(NameError::TooLong);
	}
	if name.is_empty()
		|| name == CURRENT_DIRECTORY
		|| name == PARENT_DIRECTORY
		|| name
			.iter()
			.any(|&byte| matches!(byte, POSIX_SEPARATOR | NUL))
	{
		return Err(NameError::NotComponent);
	}
	if profile == NameProfile::PortableAscii {
		if !name.is_ascii() {
			return Err(NameError::Encoding);
		}
		if name
			.iter()
			.any(|byte| byte.is_ascii_control() || WIN32_FORBIDDEN.contains(byte))
		{
			return Err(NameError::ForbiddenCharacter);
		}
		if name.first() == Some(&ASCII_SPACE)
			|| name
				.last()
				.is_some_and(|byte| PORTABLE_NAME_SUFFIXES.contains(byte))
		{
			return Err(NameError::TrimmedName);
		}
		if is_device_name(name) {
			return Err(NameError::DeviceName);
		}
	}
	Ok(())
}

fn is_device_name(name: &[u8]) -> bool {
	let stem_end = name
		.iter()
		.position(|&byte| byte == EXTENSION_SEPARATOR)
		.unwrap_or(name.len());
	let mut stem = &name[..stem_end];
	while let [prefix @ .., ASCII_SPACE] = stem {
		stem = prefix;
	}
	WIN32_LITERAL_DEVICES
		.iter()
		.any(|reserved| stem.eq_ignore_ascii_case(reserved))
		|| stem.split_last().is_some_and(|(&number, family)| {
			number.is_ascii_digit()
				&& NUMBERED_DEVICE_PREFIXES
					.iter()
					.any(|prefix| family.eq_ignore_ascii_case(prefix))
		})
}
