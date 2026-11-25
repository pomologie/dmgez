use dmgez_vfs::volume::RawName;
use thiserror::Error;

const CURRENT_DIRECTORY: &str = ".";
const PARENT_DIRECTORY: &str = "..";
const POSIX_SEPARATOR: char = '/';
const NUL: char = '\0';
const WIN32_FORBIDDEN: &[u8] = b"<>:\"/\\|?*";
const WIN32_LITERAL_DEVICES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
const NUMBERED_DEVICE_PREFIXES: [&str; 2] = ["COM", "LPT"];
const EXTENSION_SEPARATOR: char = '.';
const ASCII_SPACE: char = ' ';
const PORTABLE_NAME_SUFFIXES: [char; 2] = [ASCII_SPACE, EXTENSION_SEPARATOR];

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
		if name.len() > max_bytes {
			return Err(NameError::TooLong);
		}
		if name.is_empty()
			|| matches!(name.as_str(), CURRENT_DIRECTORY | PARENT_DIRECTORY)
			|| name.contains([POSIX_SEPARATOR, NUL])
		{
			return Err(NameError::NotComponent);
		}
		if profile == NameProfile::PortableAscii {
			if !name.is_ascii() {
				return Err(NameError::Encoding);
			}
			if name
				.bytes()
				.any(|byte| byte.is_ascii_control() || WIN32_FORBIDDEN.contains(&byte))
			{
				return Err(NameError::ForbiddenCharacter);
			}
			if name.starts_with(ASCII_SPACE) || name.ends_with(PORTABLE_NAME_SUFFIXES) {
				return Err(NameError::TrimmedName);
			}
			if is_device_name(&name) {
				return Err(NameError::DeviceName);
			}
		}
		Ok(Self(name))
	}

	pub(super) fn comparison_key(&self, profile: NameProfile) -> String {
		match profile {
			NameProfile::PortableAscii => self.0.to_ascii_lowercase(),
			NameProfile::PosixUtf8 => self.0.clone(),
		}
	}
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

fn is_device_name(name: &str) -> bool {
	let stem = name
		.split_once(EXTENSION_SEPARATOR)
		.map_or(name, |(stem, _)| stem)
		.trim_end_matches(ASCII_SPACE);
	WIN32_LITERAL_DEVICES
		.iter()
		.any(|reserved| stem.eq_ignore_ascii_case(reserved))
		|| stem
			.as_bytes()
			.split_last()
			.is_some_and(|(&number, family)| {
				number.is_ascii_digit()
					&& NUMBERED_DEVICE_PREFIXES
						.iter()
						.any(|prefix| family.eq_ignore_ascii_case(prefix.as_bytes()))
			})
}
