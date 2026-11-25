//! Read-only translation of the current appdmg JSON specification.

use std::{
	collections::BTreeMap,
	fmt,
	path::{Component, Path, PathBuf},
};

use serde::Serialize;
use thiserror::Error;

use crate::{
	ConfigError, MAX_MANIFEST_BYTES, Project, SCHEMA_VERSION, VirtualPath,
	parse::{VARIABLE_END, VARIABLE_START},
	value::{PATH_SEPARATOR, URI_AUTHORITY},
};

mod input;

use input::{EntryKind, Extent, Filesystem, Format, Input, InputEntry, Object, Point};

const DEFAULT_ICON_SIZE: i64 = 80;
const DEFAULT_WINDOW_SIZE: [i64; 2] = [640, 480];
const ZLIB_LEVEL: u8 = 9;
const LITERAL_NAME_PREFIX: &str = "APPDMG_LITERAL_";
const NUL: char = '\0';

/// A schema-v1 project document and any explicit migration diagnostics.
#[derive(Debug)]
#[non_exhaustive]
pub struct AppDmgImport {
	pub manifest: String,
	pub diagnostics: Vec<AppDmgDiagnostic>,
}

/// Appdmg settings that require a choice after translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AppDmgDiagnostic {
	/// An image without an explicit window size used host image dimensions.
	BackgroundWindowSize,
	/// Implicit Retina siblings were not looked up or combined.
	RetinaBackground,
	/// The image takes precedence when both background fields are present.
	BackgroundColorShadowed,
}

impl fmt::Display for AppDmgDiagnostic {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::BackgroundWindowSize => {
				"set the Finder window size explicitly to retain appdmg image-based sizing"
			}
			Self::RetinaBackground => {
				"set the Retina background source explicitly when the appdmg project used a Retina sibling"
			}
			Self::BackgroundColorShadowed => "the background image takes precedence over the color",
		})
	}
}

/// A malformed or unsupported appdmg project or an invalid translation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AppDmgImportError {
	#[error("appdmg JSON exceeds the {MAX_MANIFEST_BYTES}-byte input limit")]
	InputLimit,
	#[error("appdmg source directory must be an absolute UTF-8 path")]
	SourceDirectory,
	#[error("invalid or unsupported appdmg JSON")]
	Json(#[from] serde_json::Error),
	#[error("appdmg signing settings require explicit dmgez release configuration")]
	Signing,
	#[error("appdmg content entry {index} must name one image-root entry")]
	Destination { index: usize },
	#[error("appdmg content entry {index} positions an undeclared image entry")]
	PositionTarget { index: usize },
	#[error("appdmg source must be a local UTF-8 path")]
	SourcePath,
	#[error("translated project exceeds the {MAX_MANIFEST_BYTES}-byte input limit")]
	OutputLimit,
	#[error("could not serialize the translated dmgez project")]
	Serialize(#[from] toml::ser::Error),
	#[error("translated appdmg settings do not form a valid dmgez project")]
	Project(#[from] ConfigError),
}

/// Translates an appdmg JSON document and validates the generated TOML with
/// [`Project::parse`]. Relative sources, the volume icon, and the background
/// image are resolved lexically against `source_directory`, the absolute
/// directory of the original JSON file. Symbolic link targets are preserved
/// verbatim. `image_output` is written as supplied and follows the generated
/// manifest's normal output path rules.
///
/// Translation does no filesystem I/O. It preserves format and filesystem
/// intent independently of the capabilities of any installed builder.
///
/// # Errors
/// Rejects invalid or unsupported JSON fields, unsafe or unrepresentable
/// paths, non-absolute source directories and invalid dmgez settings.
pub fn import_appdmg(
	json: &str,
	source_directory: &Path,
	image_output: &str,
) -> Result<AppDmgImport, AppDmgImportError> {
	if json.len() > MAX_MANIFEST_BYTES {
		return Err(AppDmgImportError::InputLimit);
	}
	if !source_directory.is_absolute() || source_directory.to_str().is_none() {
		return Err(AppDmgImportError::SourceDirectory);
	}
	if source_directory.as_os_str().len() > MAX_MANIFEST_BYTES
		|| image_output.len() > MAX_MANIFEST_BYTES
	{
		return Err(AppDmgImportError::OutputLimit);
	}
	let Object(input): Object<Input> = serde_json::from_str(json)?;
	if input.code_sign.is_some() {
		return Err(AppDmgImportError::Signing);
	}

	let mut literals = Literals::new();
	let mut diagnostics = Vec::new();
	let finder = finder(&input, source_directory, &mut literals, &mut diagnostics)?;
	let volume_icon = input
		.icon
		.filter(|path| !path.is_empty())
		.map(|path| {
			literals
				.preserve(resolve_source(source_directory, &path)?)
				.map(|source| VolumeIcon { source })
		})
		.transpose()?;
	let contents = contents(input.contents, source_directory, &mut literals)?;
	let format = input.format.unwrap_or_default();
	let translated = Manifest {
		schema_version: SCHEMA_VERSION,
		image: Image {
			name: literals.preserve(input.title)?,
			output: literals.preserve(image_output.to_owned())?,
			format,
			filesystem: input.filesystem.unwrap_or_default(),
		},
		finder,
		compression: (format == Format::Udzo).then_some(Compression { level: ZLIB_LEVEL }),
		volume_icon,
		contents,
		defines: literals.defines,
	};
	let manifest = toml::to_string_pretty(&translated)?;
	if manifest.len() > MAX_MANIFEST_BYTES {
		return Err(AppDmgImportError::OutputLimit);
	}
	Project::parse(&manifest)?;
	Ok(AppDmgImport {
		manifest,
		diagnostics,
	})
}

#[derive(Serialize)]
struct Manifest {
	schema_version: i64,
	image: Image,
	finder: Finder,
	#[serde(skip_serializing_if = "Option::is_none")]
	compression: Option<Compression>,
	#[serde(skip_serializing_if = "Option::is_none")]
	volume_icon: Option<VolumeIcon>,
	contents: Vec<Content>,
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	defines: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct Image {
	name: String,
	output: String,
	format: Format,
	filesystem: Filesystem,
}

#[derive(Serialize)]
struct Finder {
	#[serde(skip_serializing_if = "Option::is_none")]
	window_position: Option<[i64; 2]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	window_size: Option<[i64; 2]>,
	#[serde(skip_serializing_if = "Option::is_none")]
	background: Option<Background>,
	icon_view: Icons,
}

#[derive(Serialize)]
struct Icons {
	icon_size: i64,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Background {
	Image { source: String },
	Color { value: String },
}

#[derive(Serialize)]
struct Compression {
	level: u8,
}

#[derive(Serialize)]
struct VolumeIcon {
	source: String,
}

#[derive(Serialize)]
struct Content {
	destination: String,
	position: [i64; 2],
	#[serde(flatten)]
	payload: Payload,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Payload {
	Copy { source: String },
	Symlink { target: String },
}

fn finder(
	input: &Input,
	directory: &Path,
	literals: &mut Literals,
	diagnostics: &mut Vec<AppDmgDiagnostic>,
) -> Result<Finder, AppDmgImportError> {
	let window_position = input
		.window
		.as_ref()
		.and_then(|window| window.0.position)
		.map(|Object(Point { x, y })| [x.0, y.0]);
	let mut window_size = input
		.window
		.as_ref()
		.and_then(|window| window.0.size)
		.map(|Object(Extent { width, height })| [width.0, height.0]);
	let background = if let Some(path) = input.background.as_deref().filter(|path| !path.is_empty())
	{
		diagnostics.push(AppDmgDiagnostic::RetinaBackground);
		if window_size.is_none() {
			diagnostics.push(AppDmgDiagnostic::BackgroundWindowSize);
		}
		if input.background_color.is_some() {
			diagnostics.push(AppDmgDiagnostic::BackgroundColorShadowed);
		}
		Some(Background::Image {
			source: literals.preserve(resolve_source(directory, path)?)?,
		})
	} else {
		window_size.get_or_insert(DEFAULT_WINDOW_SIZE);
		input
			.background_color
			.as_ref()
			.map(|value| {
				literals
					.preserve(value.clone())
					.map(|value| Background::Color { value })
			})
			.transpose()?
	};
	let icon_size = input
		.icon_size
		.map(|size| size.0)
		.filter(|&size| size != 0)
		.unwrap_or(DEFAULT_ICON_SIZE);
	Ok(Finder {
		window_position,
		window_size,
		background,
		icon_view: Icons { icon_size },
	})
}

fn contents(
	entries: Vec<Object<InputEntry>>,
	directory: &Path,
	literals: &mut Literals,
) -> Result<Vec<Content>, AppDmgImportError> {
	let mut positions = BTreeMap::new();
	let mut payloads = Vec::new();
	let mut position_targets = Vec::new();
	for (index, Object(entry)) in entries.into_iter().enumerate() {
		let name = entry
			.name
			.as_deref()
			.filter(|name| !name.is_empty())
			.unwrap_or_else(|| {
				entry
					.path
					.trim_end_matches(PATH_SEPARATOR)
					.rsplit(PATH_SEPARATOR)
					.next()
					.unwrap_or_default()
			});
		let destination = format!("{PATH_SEPARATOR}{name}");
		if name.is_empty()
			|| name.contains(PATH_SEPARATOR)
			|| destination.parse::<VirtualPath>().is_err()
		{
			return Err(AppDmgImportError::Destination { index });
		}
		positions.insert(destination.clone(), [entry.x.0, entry.y.0]);
		let payload = match entry.kind {
			EntryKind::Position => {
				position_targets.push((index, destination));
				continue;
			}
			EntryKind::File => Payload::Copy {
				source: literals.preserve(resolve_source(directory, &entry.path)?)?,
			},
			EntryKind::Link => Payload::Symlink {
				target: literals.preserve(entry.path)?,
			},
		};
		payloads.push((destination, payload));
	}
	let declared: std::collections::BTreeSet<_> = payloads.iter().map(|(name, _)| name).collect();
	for (index, destination) in &position_targets {
		if !declared.contains(destination) {
			return Err(AppDmgImportError::PositionTarget { index: *index });
		}
	}

	payloads
		.into_iter()
		.map(|(destination, payload)| {
			let position = positions[&destination];
			Ok(Content {
				destination: literals.preserve(destination)?,
				position,
				payload,
			})
		})
		.collect()
}

fn resolve_source(directory: &Path, source: &str) -> Result<String, AppDmgImportError> {
	if source.is_empty() || source.contains(NUL) || source.contains(URI_AUTHORITY) {
		return Err(AppDmgImportError::SourcePath);
	}
	let absolute = directory.join(source);
	if !absolute.is_absolute() {
		return Err(AppDmgImportError::SourcePath);
	}
	let mut normalized = PathBuf::new();
	for component in absolute.components() {
		match component {
			Component::CurDir => {}
			Component::ParentDir => {
				normalized.pop();
			}
			_ => normalized.push(component),
		}
	}
	normalized
		.into_os_string()
		.into_string()
		.map_err(|_| AppDmgImportError::SourcePath)
}

struct Literals {
	defines: BTreeMap<String, String>,
	remaining: usize,
}

impl Literals {
	fn new() -> Self {
		Self {
			defines: BTreeMap::new(),
			remaining: MAX_MANIFEST_BYTES,
		}
	}

	fn preserve(&mut self, value: String) -> Result<String, AppDmgImportError> {
		self.remaining = self
			.remaining
			.checked_sub(value.len())
			.ok_or(AppDmgImportError::OutputLimit)?;
		if !value.contains(VARIABLE_START) {
			return Ok(value);
		}
		// dmgez substitutes once. A literal reference marker in imported text
		// is data, so reference a document-local definition containing it.
		let key = format!("{LITERAL_NAME_PREFIX}{}", self.defines.len());
		let reference = format!("{VARIABLE_START}{key}{VARIABLE_END}");
		self.defines.insert(key, value);
		Ok(reference)
	}
}
