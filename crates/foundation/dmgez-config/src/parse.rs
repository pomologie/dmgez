use std::{collections::BTreeMap, path::Path};

use thiserror::Error;
use toml::{Table, Value};

use crate::{
	CompressionError, ConfigOverride, Content, ContentError, ImageError, OverrideError,
	PerformanceError, Project, VolumeIcon, VolumeIconError, content::validate_destinations,
	model::RawProject, overrides,
};

/// The version understood by this parser.
pub const SCHEMA_VERSION: i64 = 1;
/// The largest input document accepted by the parser.
pub const MAX_MANIFEST_BYTES: usize = 16 << 20;
const MAX_EXPANDED_BYTES: usize = 64 << 20;
const MAX_VALUE_DEPTH: usize = 128;
const VERSION_KEY: &str = "schema_version";
const DEFINES_KEY: &str = "defines";
pub(crate) const VARIABLE_START: &str = "${";
pub(crate) const VARIABLE_END: char = '}';
const IDENTIFIER_SEPARATOR: u8 = b'_';

/// Errors that prevent admission of a project manifest.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
	#[error("manifest exceeds the {MAX_MANIFEST_BYTES}-byte input limit")]
	TooLarge,
	#[error("invalid TOML (byte offset {offset:?})")]
	Syntax { offset: Option<usize> },
	#[error("manifest requires an integer schema_version")]
	SchemaVersion,
	#[error("unsupported manifest schema version {0}")]
	UnsupportedVersion(i64),
	#[error("manifest exceeds the maximum TOML nesting depth")]
	Nesting,
	#[error("manifest fields do not match the project schema")]
	Schema,
	#[error(transparent)]
	Define(#[from] DefineError),
	#[error(transparent)]
	Override(#[from] OverrideError),
	#[error("invalid image settings: {0}")]
	Image(#[from] ImageError),
	#[error("invalid compression settings: {0}")]
	Compression(#[from] CompressionError),
	#[error("invalid performance settings: {0}")]
	Performance(#[from] PerformanceError),
	#[error("invalid content entry at zero-based index {index}: {error}")]
	Content { index: usize, error: ContentError },
	#[error("invalid volume icon settings: {0}")]
	VolumeIcon(#[from] VolumeIconError),
}

/// Invalid variable definitions or references.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum DefineError {
	#[error("defines must be a table containing only strings")]
	InvalidTable,
	#[error("variable names must be ASCII identifiers")]
	InvalidName,
	#[error("unterminated variable reference")]
	Unterminated,
	#[error("reference to an undefined variable")]
	UnknownVariable,
	#[error("expanded manifest strings exceed the {MAX_EXPANDED_BYTES}-byte limit")]
	ExpansionLimit,
}

impl Project {
	/// Parses TOML and validates the resulting project without performing I/O.
	///
	/// # Errors
	///
	/// Returns a configuration error for unsupported schema versions, malformed
	/// input, undefined variables, or invalid project settings.
	pub fn parse(input: &str) -> Result<Self, ConfigError> {
		Self::parse_with_defines(input, &BTreeMap::new())
	}

	/// Parses TOML with explicit variable definitions taking precedence over
	/// the values in the document. Substitution applies once to string values.
	/// Definition values are literal text and table keys are never interpolated.
	///
	/// # Errors
	///
	/// Returns a configuration error for invalid definitions, references or
	/// manifest settings.
	pub fn parse_with_defines(
		input: &str,
		defines: &BTreeMap<String, String>,
	) -> Result<Self, ConfigError> {
		Self::parse_with_overrides(input, defines, &[], Path::new(""))
	}

	/// Parses a manifest with explicit definitions and typed scalar overrides.
	/// Explicit definitions replace manifest definitions during substitution.
	/// Overrides replace substituted TOML values in input order, with the last
	/// assignment winning for a repeated key. The effective project receives the
	/// same schema and semantic validation as [`Self::parse`].
	///
	/// Only relative override paths use `working_directory`, which must be
	/// absolute for those paths. Paths originating in TOML retain their original
	/// config-relative semantics. No filesystem access occurs during parsing.
	/// # Errors
	/// Returns a configuration error for invalid TOML, substitutions, effective
	/// settings, or override paths that cannot be anchored to the working directory.
	pub fn parse_with_overrides(
		input: &str,
		defines: &BTreeMap<String, String>,
		assignments: &[ConfigOverride],
		working_directory: &Path,
	) -> Result<Self, ConfigError> {
		if input.len() > MAX_MANIFEST_BYTES {
			return Err(ConfigError::TooLarge);
		}
		let mut document: Table = toml::from_str(input).map_err(|error| ConfigError::Syntax {
			offset: error.span().map(|span| span.start),
		})?;
		let version = document
			.remove(VERSION_KEY)
			.and_then(|value| value.as_integer())
			.ok_or(ConfigError::SchemaVersion)?;
		if version != SCHEMA_VERSION {
			return Err(ConfigError::UnsupportedVersion(version));
		}

		let local_defines = document.remove(DEFINES_KEY);
		let variables = Variables::new(local_defines.as_ref(), defines)?;
		let mut remaining_bytes = MAX_EXPANDED_BYTES;
		for (_, value) in &mut document {
			variables.expand(value, 0, &mut remaining_bytes)?;
		}
		overrides::apply(&mut document, assignments, remaining_bytes)?;
		let mut raw: RawProject = document.try_into().map_err(|_| ConfigError::Schema)?;
		overrides::anchor_paths(&mut raw, assignments, working_directory)?;
		Self::try_from(raw)
	}
}

impl TryFrom<RawProject> for Project {
	type Error = ConfigError;

	fn try_from(raw: RawProject) -> Result<Self, ConfigError> {
		raw.image.validate()?;
		raw.compression.validate(raw.image.format)?;
		raw.performance.validate()?;
		let contents = raw
			.contents
			.into_iter()
			.enumerate()
			.map(|(index, entry)| {
				Content::try_from(entry).map_err(|error| ConfigError::Content { index, error })
			})
			.collect::<Result<Vec<_>, _>>()?;
		validate_destinations(&contents)
			.map_err(|(index, error)| ConfigError::Content { index, error })?;
		let volume_icon = raw.volume_icon.map(VolumeIcon::try_from).transpose()?;

		Ok(Self {
			image: raw.image,
			contents,
			compression: raw.compression,
			performance: raw.performance,
			metadata: raw.metadata,
			build: raw.build,
			finder: raw.finder,
			volume_icon,
			signing: raw.signing,
			notarization: raw.notarization,
			release: raw.release,
			license: raw.license,
		})
	}
}

struct Variables<'a> {
	local: Option<&'a Table>,
	explicit: &'a BTreeMap<String, String>,
}

impl<'a> Variables<'a> {
	fn new(
		local: Option<&'a Value>,
		explicit: &'a BTreeMap<String, String>,
	) -> Result<Self, DefineError> {
		let local = local
			.map(|value| value.as_table().ok_or(DefineError::InvalidTable))
			.transpose()?;
		if let Some(table) = local {
			for (key, value) in table {
				validate_identifier(key)?;
				if !value.is_str() {
					return Err(DefineError::InvalidTable);
				}
			}
		}
		for key in explicit.keys() {
			validate_identifier(key)?;
		}
		Ok(Self { local, explicit })
	}

	fn get(&self, name: &str) -> Result<&str, DefineError> {
		self.explicit
			.get(name)
			.map(String::as_str)
			.or_else(|| self.local?.get(name)?.as_str())
			.ok_or(DefineError::UnknownVariable)
	}

	fn expand(
		&self,
		value: &mut Value,
		depth: usize,
		remaining_bytes: &mut usize,
	) -> Result<(), ConfigError> {
		if depth > MAX_VALUE_DEPTH {
			return Err(ConfigError::Nesting);
		}
		match value {
			Value::String(text) => self.expand_string(text, remaining_bytes)?,
			Value::Array(array) => {
				for item in array {
					self.expand(item, depth + 1, remaining_bytes)?;
				}
			}
			Value::Table(table) => {
				for (_, entry) in table {
					self.expand(entry, depth + 1, remaining_bytes)?;
				}
			}
			_ => {}
		}
		Ok(())
	}

	fn expand_string(
		&self,
		text: &mut String,
		remaining_bytes: &mut usize,
	) -> Result<(), DefineError> {
		if !text.contains(VARIABLE_START) {
			return reserve_bytes(remaining_bytes, text.len());
		}
		let mut pending = text.as_str();
		let mut output = String::new();
		while let Some((prefix, reference)) = pending.split_once(VARIABLE_START) {
			let (name, suffix) = reference
				.split_once(VARIABLE_END)
				.ok_or(DefineError::Unterminated)?;
			validate_identifier(name)?;
			let replacement = self.get(name)?;
			reserve_bytes(remaining_bytes, prefix.len())?;
			reserve_bytes(remaining_bytes, replacement.len())?;
			output.push_str(prefix);
			output.push_str(replacement);
			pending = suffix;
		}
		reserve_bytes(remaining_bytes, pending.len())?;
		output.push_str(pending);
		*text = output;
		Ok(())
	}
}

fn reserve_bytes(remaining: &mut usize, bytes: usize) -> Result<(), DefineError> {
	*remaining = remaining
		.checked_sub(bytes)
		.ok_or(DefineError::ExpansionLimit)?;
	Ok(())
}

fn validate_identifier(name: &str) -> Result<(), DefineError> {
	let mut bytes = name.bytes();
	if bytes
		.next()
		.is_some_and(|byte| byte.is_ascii_alphabetic() || byte == IDENTIFIER_SEPARATOR)
		&& bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == IDENTIFIER_SEPARATOR)
	{
		return Ok(());
	}
	Err(DefineError::InvalidName)
}
