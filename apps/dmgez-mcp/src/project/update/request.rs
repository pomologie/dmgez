use dmgez_config::{
	Coordinate, VirtualPath,
	project_edit::{ContentChanges, ContentDraft, FieldChange, ProjectEdit},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Deserializer, de::Error as _};

use crate::{local::MAX_FILE_NAME_BYTES, project::read::SHA256_HEX_BYTES};

pub(crate) const MAX_PROJECT_EDITS: usize = 128;
const REVISION_PATTERN: &str = "^[0-9a-f]+$";
const ABSOLUTE_IMAGE_PATH_PATTERN: &str = "^/";

#[derive(Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct UpdateRequest(#[serde(deserialize_with = "deserialize_arguments")] Arguments);

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Arguments {
	/// TOML file name relative to the configured project root.
	#[schemars(length(min = 1, max = MAX_FILE_NAME_BYTES))]
	file_name: String,
	/// Lowercase SHA-256 of the exact original bytes, as returned by project_read.
	#[schemars(length(equal = SHA256_HEX_BYTES), regex(pattern = REVISION_PATTERN))]
	expected_revision_sha256: String,
	/// Ordered changes referencing the project at the admitted revision.
	#[schemars(length(min = 1, max = MAX_PROJECT_EDITS))]
	edits: Vec<Edit>,
}

impl UpdateRequest {
	pub(super) fn file_name(&self) -> &str {
		&self.0.file_name
	}

	pub(super) fn expected_revision_sha256(&self) -> &str {
		&self.0.expected_revision_sha256
	}

	pub(super) fn into_edits(self) -> Vec<ProjectEdit> {
		self.0.edits.into_iter().map(ProjectEdit::from).collect()
	}
}

fn deserialize_arguments<'de, D>(deserializer: D) -> Result<Arguments, D::Error>
where
	D: Deserializer<'de>,
{
	// Serde's errors can include arbitrarily long keys and values supplied by the client.
	let arguments = Arguments::deserialize(deserializer)
		.map_err(|_| D::Error::custom("invalid project update arguments"))?;
	let digest = &arguments.expected_revision_sha256;
	if digest.len() != SHA256_HEX_BYTES
		|| !digest
			.bytes()
			.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
	{
		return Err(D::Error::custom("expected a lowercase SHA-256 revision"));
	}
	if !(1..=MAX_PROJECT_EDITS).contains(&arguments.edits.len()) {
		return Err(D::Error::custom(
			"project update edit count is out of range",
		));
	}
	Ok(arguments)
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
	AddCopy {
		source: String,
		#[schemars(with = "Option<String>", regex(pattern = ABSOLUTE_IMAGE_PATH_PATTERN))]
		destination: Option<VirtualPath>,
	},
	AddDirectory {
		#[schemars(with = "String", regex(pattern = ABSOLUTE_IMAGE_PATH_PATTERN))]
		destination: VirtualPath,
	},
	AddSymlink {
		#[schemars(with = "String", regex(pattern = ABSOLUTE_IMAGE_PATH_PATTERN))]
		destination: VirtualPath,
		target: String,
	},
	Modify {
		#[schemars(with = "String", regex(pattern = ABSOLUTE_IMAGE_PATH_PATTERN))]
		target: VirtualPath,
		#[schemars(with = "Option<String>", regex(pattern = ABSOLUTE_IMAGE_PATH_PATTERN))]
		destination: Option<VirtualPath>,
		/// Omit to keep, null to clear, or supply two coordinates.
		#[serde(default, deserialize_with = "deserialize_change")]
		#[schemars(with = "Option<[f64; 2]>")]
		position: FieldChange<[Coordinate; 2]>,
		/// Omit to keep, null to clear, or supply a boolean.
		#[serde(default, deserialize_with = "deserialize_change")]
		#[schemars(with = "Option<bool>")]
		hidden: FieldChange<bool>,
		/// Omit to keep, null to clear, or supply a boolean.
		#[serde(default, deserialize_with = "deserialize_change")]
		#[schemars(with = "Option<bool>")]
		hide_extension: FieldChange<bool>,
	},
	Remove {
		#[schemars(with = "String", regex(pattern = ABSOLUTE_IMAGE_PATH_PATTERN))]
		target: VirtualPath,
	},
	SetImageName {
		name: String,
	},
}

impl From<Edit> for ProjectEdit {
	fn from(edit: Edit) -> Self {
		match edit {
			Edit::AddCopy {
				source,
				destination,
			} => Self::Add(ContentDraft::Copy {
				source,
				destination,
			}),
			Edit::AddDirectory { destination } => {
				Self::Add(ContentDraft::Directory { destination })
			}
			Edit::AddSymlink {
				destination,
				target,
			} => Self::Add(ContentDraft::Symlink {
				destination,
				target,
			}),
			Edit::Modify {
				target,
				destination,
				position,
				hidden,
				hide_extension,
			} => {
				let mut changes = ContentChanges::default();
				changes.destination = destination;
				changes.position = match position {
					FieldChange::Keep => FieldChange::Keep,
					FieldChange::Clear => FieldChange::Clear,
					FieldChange::Set(point) => FieldChange::Set(point.map(Coordinate::get)),
				};
				changes.hidden = hidden;
				changes.hide_extension = hide_extension;
				Self::Modify { target, changes }
			}
			Edit::Remove { target } => Self::Remove { target },
			Edit::SetImageName { name } => Self::SetImageName(name),
		}
	}
}

fn deserialize_change<'de, D, T>(deserializer: D) -> Result<FieldChange<T>, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de>,
{
	Ok(Option::<T>::deserialize(deserializer)?.map_or(FieldChange::Clear, FieldChange::Set))
}
