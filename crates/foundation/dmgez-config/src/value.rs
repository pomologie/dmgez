use std::{
	fmt,
	path::{Path, PathBuf},
	str::FromStr,
};

use serde::{Deserialize, Deserializer, de};
use thiserror::Error;

const BYTES_PER_KIB: u64 = 1 << 10;
const IEC_UNITS: [(&str, u64); 5] = [
	("KiB", BYTES_PER_KIB),
	("MiB", BYTES_PER_KIB.pow(2)),
	("GiB", BYTES_PER_KIB.pow(3)),
	("TiB", BYTES_PER_KIB.pow(4)),
	("B", 1),
];
const BYTES_PER_KB: u64 = 1_000;
const DECIMAL_SIZE_UNITS: [(&str, u64); 4] = [
	("k", BYTES_PER_KB),
	("m", BYTES_PER_KB.pow(2)),
	("g", BYTES_PER_KB.pow(3)),
	("t", BYTES_PER_KB.pow(4)),
];
const AUTO: &str = "auto";
pub(crate) const PATH_SEPARATOR: char = '/';
const CURRENT_DIRECTORY: &str = ".";
const PARENT_DIRECTORY: &str = "..";
pub(crate) const URI_AUTHORITY: &str = "://";
const FORBIDDEN_COMPONENT_CHARACTERS: [char; 3] = ['\0', '\\', ':'];

/// A count of bytes expressed in configuration as an integer or an IEC size.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Bytes(u64);

impl Bytes {
	/// Returns the number of bytes.
	#[must_use]
	pub const fn get(self) -> u64 {
		self.0
	}

	fn scaled(digits: &str, unit: u64) -> Result<Self, SizeError> {
		if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
			return Err(SizeError::Invalid);
		}
		let count = digits.parse::<u64>().map_err(|_| SizeError::Overflow)?;
		count.checked_mul(unit).map(Self).ok_or(SizeError::Overflow)
	}
}

impl FromStr for Bytes {
	type Err = SizeError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		let (digits, unit) = IEC_UNITS
			.iter()
			.find_map(|(suffix, unit)| value.strip_suffix(suffix).map(|digits| (digits, unit)))
			.ok_or(SizeError::Invalid)?;
		Self::scaled(digits, *unit)
	}
}

/// An invalid or overflowing byte quantity.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum SizeError {
	/// A decimal count and a recognized IEC suffix are required.
	#[error("expected a byte count with an IEC suffix (B, KiB, MiB, GiB, TiB)")]
	Invalid,
	/// The text does not specify a supported image capacity.
	#[error(
		"expected auto or a byte count with an optional suffix (B, KiB, MiB, GiB, TiB, k, m, g, t)"
	)]
	InvalidImage,
	/// The value does not fit in 64 bits.
	#[error("byte size exceeds the 64-bit limit")]
	Overflow,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ByteInput {
	Integer(u64),
	Text(String),
}

impl ByteInput {
	fn bytes(self) -> Result<Bytes, SizeError> {
		match self {
			Self::Integer(count) => Ok(Bytes(count)),
			Self::Text(value) => value.parse(),
		}
	}
}

impl<'de> Deserialize<'de> for Bytes {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		ByteInput::deserialize(deserializer)?
			.bytes()
			.map_err(de::Error::custom)
	}
}

/// The `.toml` setting used to choose image capacity before applying shrink policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Size {
	/// Derive capacity from the complete image plan.
	#[default]
	Auto,
	/// An explicit byte count.
	Fixed(Bytes),
}

impl FromStr for Size {
	type Err = SizeError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		if value == AUTO {
			return Ok(Self::Auto);
		}
		let decimal = DECIMAL_SIZE_UNITS
			.iter()
			.find_map(|(suffix, unit)| value.strip_suffix(suffix).map(|digits| (digits, *unit)));
		let bytes = match decimal {
			Some((digits, unit)) => Bytes::scaled(digits, unit),
			None if value.bytes().all(|digit| digit.is_ascii_digit()) => Bytes::scaled(value, 1),
			None => value.parse(),
		};
		bytes.map(Self::Fixed).map_err(|error| match error {
			SizeError::Invalid => SizeError::InvalidImage,
			error => error,
		})
	}
}

impl<'de> Deserialize<'de> for Size {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		match ByteInput::deserialize(deserializer)? {
			ByteInput::Integer(count) => Ok(Self::Fixed(Bytes(count))),
			ByteInput::Text(value) => value.parse().map_err(de::Error::custom),
		}
	}
}

/// A user-specified Boolean or a policy selected by the active backend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AutoFlag {
	/// Select automatically.
	#[default]
	Auto,
	/// Enable explicitly.
	Enabled,
	/// Disable explicitly.
	Disabled,
}

impl<'de> Deserialize<'de> for AutoFlag {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		#[derive(Deserialize)]
		#[serde(untagged)]
		enum Input {
			Flag(bool),
			Text(String),
		}
		match Input::deserialize(deserializer)? {
			Input::Flag(true) => Ok(Self::Enabled),
			Input::Flag(false) => Ok(Self::Disabled),
			Input::Text(mode) if mode == AUTO => Ok(Self::Auto),
			Input::Text(_) => Err(de::Error::custom("expected a Boolean or auto")),
		}
	}
}

/// A name or string setting that must contain at least one character and no NUL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Text(String);

impl Text {
	/// Returns the validated value.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl<'de> Deserialize<'de> for Text {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = String::deserialize(deserializer)?;
		validate_text(&value).map_err(de::Error::custom)?;
		Ok(Self(value))
	}
}

fn validate_text(value: &str) -> Result<(), &'static str> {
	if value.is_empty() || value.contains('\0') {
		return Err("expected nonempty text without NUL");
	}
	Ok(())
}

/// A host-local path as written by the user. It is never opened by this crate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalPath(PathBuf);

impl LocalPath {
	pub(crate) fn parse_literal(value: &str) -> Result<Self, &'static str> {
		validate_local_path(value)?;
		Ok(Self(value.into()))
	}

	/// Borrows the original host path.
	#[must_use]
	pub fn as_path(&self) -> &Path {
		&self.0
	}

	/// Resolves a relative path against the caller's configuration directory.
	/// The platform's ordinary path joining rules apply. No I/O is performed.
	#[must_use]
	pub fn resolve(&self, directory: &Path) -> PathBuf {
		directory.join(&self.0)
	}

	pub(crate) fn anchor(&mut self, directory: &Path) {
		self.0 = self.resolve(directory);
	}
}

impl<'de> Deserialize<'de> for LocalPath {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = String::deserialize(deserializer)?;
		validate_local_path(&value).map_err(de::Error::custom)?;
		Ok(Self(PathBuf::from(value)))
	}
}

fn validate_local_path(value: &str) -> Result<(), &'static str> {
	validate_text(value)?;
	if value.contains(URI_AUTHORITY) {
		return Err("expected a local path, not a URL");
	}
	Ok(())
}

/// An absolute slash-delimited path within an image. It has no host path semantics.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct VirtualPath(String);

impl VirtualPath {
	/// Borrows the canonical path, including the leading slash.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}

	/// Returns whether the path identifies the image root.
	#[must_use]
	pub fn is_root(&self) -> bool {
		self.0.len() == PATH_SEPARATOR.len_utf8()
	}

	/// Iterates over path components in order from the image root.
	pub fn components(&self) -> impl Iterator<Item = &str> {
		self.0
			.split(PATH_SEPARATOR)
			.skip(1)
			.filter(|component| !component.is_empty())
	}
}

impl fmt::Display for VirtualPath {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

impl FromStr for VirtualPath {
	type Err = VirtualPathError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		let relative = value
			.strip_prefix(PATH_SEPARATOR)
			.ok_or(VirtualPathError::Relative)?;
		if !relative.is_empty() {
			validate_components(relative)?;
		}
		Ok(Self(value.to_owned()))
	}
}

impl<'de> Deserialize<'de> for VirtualPath {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		String::deserialize(deserializer)?
			.parse()
			.map_err(de::Error::custom)
	}
}

pub(crate) fn validate_components(relative: &str) -> Result<(), VirtualPathError> {
	if relative.split(PATH_SEPARATOR).any(|component| {
		component.is_empty()
			|| matches!(component, CURRENT_DIRECTORY | PARENT_DIRECTORY)
			|| component.contains(FORBIDDEN_COMPONENT_CHARACTERS)
	}) {
		return Err(VirtualPathError::InvalidComponent);
	}
	Ok(())
}

/// An invalid image path.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum VirtualPathError {
	/// The path is missing the leading slash.
	#[error("image path must start with a slash")]
	Relative,
	/// Empty names, dot components, NUL, host drive separators and ADS are outside this grammar.
	#[error("invalid image path component")]
	InvalidComponent,
}

/// A finite numeric coordinate such as a position or Finder dimension.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Coordinate(f64);

impl Coordinate {
	/// Returns the finite number.
	#[must_use]
	pub const fn get(self) -> f64 {
		self.0
	}
}

impl<'de> Deserialize<'de> for Coordinate {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = f64::deserialize(deserializer)?;
		if !value.is_finite() {
			return Err(de::Error::custom("expected a finite number"));
		}
		Ok(Self(value))
	}
}

/// A finite numeric dimension with a minimum.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Dimension<const MINIMUM: u8>(Coordinate);

impl<const MINIMUM: u8> Dimension<MINIMUM> {
	/// Returns the validated dimension.
	#[must_use]
	pub const fn get(self) -> f64 {
		self.0.get()
	}
}

impl<'de, const MINIMUM: u8> Deserialize<'de> for Dimension<MINIMUM> {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = Coordinate::deserialize(deserializer)?;
		if value.get() < f64::from(MINIMUM) {
			return Err(de::Error::custom("dimension is below its minimum"));
		}
		Ok(Self(value))
	}
}
