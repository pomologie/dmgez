use std::{
	collections::{BTreeMap, BTreeSet},
	ops::Bound::{Included, Unbounded},
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
	LocalPath, Text, VirtualPath,
	value::{Coordinate, PATH_SEPARATOR, validate_components},
};

const RESERVED_ROOT_NAMES: [&str; 3] = [".DS_Store", ".background", ".VolumeIcon.icns"];

/// One declared entry in the image.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Content {
	pub destination: VirtualPath,
	pub payload: Payload,
	pub position: Option<[Coordinate; 2]>,
	pub hidden: Option<bool>,
	pub hide_extension: Option<bool>,
	pub custom_icon: Option<LocalPath>,
}

/// The data source for an image entry.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Payload {
	Copy(CopySource),
	Directory,
	Symlink { target: Text },
}

/// A host-local source and optional filters for recursive copies.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CopySource {
	pub source: LocalPath,
	pub recursive: bool,
	pub include: Vec<Text>,
	pub exclude: Vec<Text>,
}

/// Invalid settings for a content declaration.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ContentError {
	#[error("copy entries require a source")]
	SourceRequired,
	#[error("a source with no usable basename requires an explicit destination")]
	Basename,
	#[error("directory and symlink entries require a destination")]
	DestinationRequired,
	#[error("only symlink entries may specify a target")]
	UnexpectedTarget,
	#[error("symlink entries require a target")]
	TargetRequired,
	#[error("source and recursive copy options require a copy entry")]
	UnexpectedCopySettings,
	#[error("include and exclude patterns require a recursive copy")]
	NonRecursivePatterns,
	#[error("include and exclude patterns must be unique normalized relative paths")]
	InvalidPatterns,
	#[error("a content entry may not replace the image root")]
	RootDestination,
	#[error("destination is reserved for generated image metadata")]
	ReservedDestination,
	#[error("destination is also declared by content entry {other}")]
	DuplicateDestination { other: usize },
	#[error("destination descends from the symlink declared by content entry {other}")]
	SymlinkAncestor { other: usize },
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum EntryType {
	#[default]
	Copy,
	Directory,
	Symlink,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawContent {
	#[serde(default, rename = "type")]
	entry_type: EntryType,
	source: Option<LocalPath>,
	destination: Option<VirtualPath>,
	target: Option<Text>,
	recursive: Option<bool>,
	include: Option<Vec<Text>>,
	exclude: Option<Vec<Text>>,
	hidden: Option<bool>,
	hide_extension: Option<bool>,
	position: Option<[Coordinate; 2]>,
	custom_icon: Option<LocalPath>,
}

impl TryFrom<RawContent> for Content {
	type Error = ContentError;

	fn try_from(raw: RawContent) -> Result<Self, Self::Error> {
		let (destination, payload) = match raw.entry_type {
			EntryType::Copy => {
				if raw.target.is_some() {
					return Err(ContentError::UnexpectedTarget);
				}
				let source = raw.source.ok_or(ContentError::SourceRequired)?;
				let recursive = raw.recursive.unwrap_or(true);
				if !recursive && (raw.include.is_some() || raw.exclude.is_some()) {
					return Err(ContentError::NonRecursivePatterns);
				}
				let include = patterns(raw.include)?;
				let exclude = patterns(raw.exclude)?;
				let destination = match raw.destination {
					Some(destination) => destination,
					None => default_destination(&source)?,
				};
				(
					destination,
					Payload::Copy(CopySource {
						source,
						recursive,
						include,
						exclude,
					}),
				)
			}
			EntryType::Directory | EntryType::Symlink => {
				if raw.source.is_some()
					|| raw.recursive.is_some()
					|| raw.include.is_some()
					|| raw.exclude.is_some()
				{
					return Err(ContentError::UnexpectedCopySettings);
				}
				let destination = raw.destination.ok_or(ContentError::DestinationRequired)?;
				let payload = if matches!(raw.entry_type, EntryType::Symlink) {
					Payload::Symlink {
						target: raw.target.ok_or(ContentError::TargetRequired)?,
					}
				} else {
					if raw.target.is_some() {
						return Err(ContentError::UnexpectedTarget);
					}
					Payload::Directory
				};
				(destination, payload)
			}
		};
		match destination.components().next() {
			None => return Err(ContentError::RootDestination),
			Some(root)
				if RESERVED_ROOT_NAMES
					.iter()
					.any(|name| root.eq_ignore_ascii_case(name)) =>
			{
				return Err(ContentError::ReservedDestination);
			}
			Some(_) => {}
		}
		Ok(Self {
			destination,
			payload,
			position: raw.position,
			hidden: raw.hidden,
			hide_extension: raw.hide_extension,
			custom_icon: raw.custom_icon,
		})
	}
}

fn default_destination(source: &LocalPath) -> Result<VirtualPath, ContentError> {
	let basename = source
		.as_path()
		.file_name()
		.and_then(|name| name.to_str())
		.ok_or(ContentError::Basename)?;
	format!("{PATH_SEPARATOR}{basename}")
		.parse()
		.map_err(|_| ContentError::Basename)
}

fn patterns(raw: Option<Vec<Text>>) -> Result<Vec<Text>, ContentError> {
	let patterns = raw.unwrap_or_default();
	let mut seen = BTreeSet::new();
	for pattern in &patterns {
		if validate_components(pattern.as_str()).is_err() || !seen.insert(pattern.as_str()) {
			return Err(ContentError::InvalidPatterns);
		}
	}
	Ok(patterns)
}

pub(crate) fn validate_destinations(contents: &[Content]) -> Result<(), (usize, ContentError)> {
	let mut declared = BTreeMap::new();
	for (index, entry) in contents.iter().enumerate() {
		if let Some(other) = declared.insert(entry.destination.as_str(), index) {
			return Err((index, ContentError::DuplicateDestination { other }));
		}
	}
	for (other, symlink) in contents.iter().enumerate() {
		if !matches!(symlink.payload, Payload::Symlink { .. }) {
			continue;
		}
		let prefix = format!("{}{PATH_SEPARATOR}", symlink.destination);
		if let Some((_, &index)) = declared
			.range::<str, _>((Included(prefix.as_str()), Unbounded))
			.next()
			.filter(|(path, _)| path.starts_with(&prefix))
		{
			return Err((index, ContentError::SymlinkAncestor { other }));
		}
	}
	Ok(())
}
