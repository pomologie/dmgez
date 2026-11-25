use std::{fmt, num::NonZeroU64, path::Path, str::FromStr};

use serde::de::{DeserializeOwned, value};
use thiserror::Error;
use toml::{Table, Value};

use crate::{
	AutoFlag, Bytes, ConfigError, Filesystem, ImageFormat, LocalPath, MAX_MANIFEST_BYTES,
	MetadataPolicy, Partition, Size, Text, model::RawProject,
};

const ASSIGNMENT_SEPARATOR: char = '=';
const SECTION_SEPARATOR: char = '.';
const IMAGE_OUTPUT: &str = "image.output";
const METADATA_SIDECAR: &str = "metadata.sidecar";

type ParseValue = fn(&str) -> Option<Value>;

const ALLOWED: &[(&str, ParseValue)] = &[
	("image.name", text::<Text>),
	(IMAGE_OUTPUT, text::<LocalPath>),
	("image.format", text::<ImageFormat>),
	("image.filesystem", text::<Filesystem>),
	("image.size", bytes_or_text::<Size>),
	("image.shrink", boolean),
	("image.free_space", bytes_or_text::<Bytes>),
	("image.partition", text::<Partition>),
	("compression.level", integer::<u8>),
	("compression.block_size", bytes_or_text::<Bytes>),
	("performance.parallel", auto_flag),
	("performance.threads", integer::<u64>),
	("performance.memory_limit", bytes_or_text::<Bytes>),
	("performance.max_entries", integer::<NonZeroU64>),
	("performance.max_metadata_bytes", bytes_or_text::<Bytes>),
	("performance.max_runs", integer::<NonZeroU64>),
	("metadata.policy", text::<MetadataPolicy>),
	(METADATA_SIDECAR, text::<LocalPath>),
	("metadata.preserve_permissions", boolean),
	("metadata.preserve_ownership", boolean),
	("metadata.preserve_timestamps", boolean),
	("metadata.preserve_extended_attributes", boolean),
	("metadata.preserve_resource_forks", boolean),
	("metadata.preserve_finder_info", boolean),
	("metadata.preserve_hard_links", boolean),
	("build.reproducible", boolean),
	("build.source_date_epoch", integer::<u64>),
	("build.verify_output", boolean),
	("build.overwrite", boolean),
	("build.fail_on_metadata_loss", boolean),
];

/// A typed scalar assignment for one supported version-one build setting.
/// Parsed values are omitted from the `Debug` representation.
#[derive(Clone)]
pub struct ConfigOverride {
	key: &'static str,
	value: Value,
}

impl ConfigOverride {
	/// Parses one `section.field=value` argument. Supported sections are
	/// `image`, `compression`, `performance`, `metadata`, and `build`.
	/// Text and paths are literal, including spaces and additional `=` signs.
	/// Numeric values use decimal notation or an applicable IEC suffix.
	/// The image size also accepts dmgbuild's decimal `k`, `m`, `g`, and `t` suffixes.
	/// Booleans use `true` and `false`. Automatic modes also accept `auto`.
	///
	/// # Errors
	/// Returns an error for an unknown key or a malformed or oversized argument.
	pub fn parse(argument: &str) -> Result<Self, OverrideError> {
		argument.parse()
	}

	/// The canonical allowlisted key.
	#[must_use]
	pub const fn key(&self) -> &'static str {
		self.key
	}
}

impl fmt::Debug for ConfigOverride {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("ConfigOverride")
			.field("key", &self.key)
			.finish_non_exhaustive()
	}
}

impl FromStr for ConfigOverride {
	type Err = OverrideError;

	fn from_str(argument: &str) -> Result<Self, Self::Err> {
		if argument.len() > MAX_MANIFEST_BYTES {
			return Err(OverrideError::TooLarge);
		}
		let (key, input) = argument
			.split_once(ASSIGNMENT_SEPARATOR)
			.ok_or(OverrideError::Assignment)?;
		let &(key, parse) = ALLOWED
			.iter()
			.find(|(allowed, _)| *allowed == key)
			.ok_or(OverrideError::UnknownKey)?;
		let value = parse(input).ok_or(OverrideError::Value { key })?;
		Ok(Self { key, value })
	}
}

/// An invalid explicit build override. Diagnostics include only known keys.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum OverrideError {
	#[error("expected an override in section.field=value form")]
	Assignment,
	#[error("configuration override key is not supported")]
	UnknownKey,
	#[error("invalid configuration override value for {key}")]
	Value { key: &'static str },
	#[error("override exceeds the {MAX_MANIFEST_BYTES}-byte input limit")]
	TooLarge,
	#[error("overrides exceed the remaining expanded manifest string budget")]
	Budget,
	#[error("relative override path for {key} requires an absolute working directory")]
	WorkingDirectory { key: &'static str },
}

pub(super) fn apply(
	document: &mut Table,
	assignments: &[ConfigOverride],
	mut remaining_string_bytes: usize,
) -> Result<(), ConfigError> {
	for assignment in assignments {
		if let Some(value) = assignment.value.as_str() {
			remaining_string_bytes = remaining_string_bytes
				.checked_sub(value.len())
				.ok_or(OverrideError::Budget)?;
		}
		let (section, field) = assignment
			.key
			.split_once(SECTION_SEPARATOR)
			.expect("allowlisted scalar override keys contain a section and field");
		let table = document
			.entry(section.to_owned())
			.or_insert_with(|| Value::Table(Table::new()))
			.as_table_mut()
			.ok_or(ConfigError::Schema)?;
		if matches!(table.get(field), Some(Value::Table(_) | Value::Array(_))) {
			return Err(ConfigError::Schema);
		}
		table.insert(field.to_owned(), assignment.value.clone());
	}
	Ok(())
}

pub(super) fn anchor_paths(
	project: &mut RawProject,
	assignments: &[ConfigOverride],
	working_directory: &Path,
) -> Result<(), OverrideError> {
	if assignments
		.iter()
		.any(|assignment| assignment.key == IMAGE_OUTPUT)
	{
		anchor(&mut project.image.output, IMAGE_OUTPUT, working_directory)?;
	}
	if assignments
		.iter()
		.any(|assignment| assignment.key == METADATA_SIDECAR)
		&& let Some(sidecar) = &mut project.metadata.sidecar
	{
		anchor(sidecar, METADATA_SIDECAR, working_directory)?;
	}
	Ok(())
}

fn anchor(path: &mut LocalPath, key: &'static str, directory: &Path) -> Result<(), OverrideError> {
	if path.as_path().is_absolute() {
		return Ok(());
	}
	if !directory.is_absolute() {
		return Err(OverrideError::WorkingDirectory { key });
	}
	path.anchor(directory);
	if !path.as_path().is_absolute() {
		return Err(OverrideError::Value { key });
	}
	Ok(())
}

fn text<T: DeserializeOwned>(input: &str) -> Option<Value> {
	T::deserialize(value::StrDeserializer::<value::Error>::new(input)).ok()?;
	Some(Value::String(input.to_owned()))
}

fn integer<T: DeserializeOwned>(input: &str) -> Option<Value> {
	let integer = input.parse::<i64>().ok()?;
	T::deserialize(value::I64Deserializer::<value::Error>::new(integer)).ok()?;
	Some(Value::Integer(integer))
}

fn bytes_or_text<T: DeserializeOwned>(input: &str) -> Option<Value> {
	integer::<T>(input).or_else(|| text::<T>(input))
}

fn boolean(input: &str) -> Option<Value> {
	input.parse().ok().map(Value::Boolean)
}

fn auto_flag(input: &str) -> Option<Value> {
	boolean(input).or_else(|| text::<AutoFlag>(input))
}
