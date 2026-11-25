use dmgez_vfs::volume::{LookupName, NodeHandle, NodeKind, ViewError, VolumeBackend, VolumeView};

use super::{Action, Error, InvalidPath, Name};

const SEPARATOR: char = '/';
const CURRENT_DIRECTORY: &str = ".";
const PARENT_DIRECTORY: &str = "..";
const MAX_PATH_COMPONENTS: usize = 128;
const MAX_COMPONENT_BYTES: usize = 1024;
const MAX_PATH_BYTES: usize = MAX_PATH_COMPONENTS * (MAX_COMPONENT_BYTES + 1);

pub(super) struct Located<Id> {
	pub node: NodeHandle<Id>,
	pub name: Option<Name>,
	pub kind: NodeKind,
}

impl<Id> Located<Id> {
	pub(super) fn require_directory<E>(&self) -> Result<(), Error<E>> {
		if self.kind == NodeKind::Directory {
			Ok(())
		} else {
			Err(Error::view(
				Action::Directory,
				ViewError::WrongKind {
					expected: NodeKind::Directory,
					actual: self.kind,
				},
			))
		}
	}
}

pub(super) fn resolve<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	path: &str,
) -> Result<Located<B::Id>, Error<B::Error>> {
	let tail = admit(path).map_err(Error::InvalidPath)?;
	let root = view
		.root()
		.map_err(|error| Error::view(Action::Root, error))?;
	let mut result = Located {
		node: root,
		name: None,
		kind: NodeKind::Directory,
	};
	if tail.is_empty() {
		return Ok(result);
	}
	let mut parts = tail.split(SEPARATOR).enumerate().peekable();
	while let Some((index, name)) = parts.next() {
		let component = index + 1;
		let entry = view
			.lookup(&result.node, LookupName::Utf8(name))
			.map_err(|error| Error::view(Action::Lookup { component }, error))?
			.ok_or(Error::MissingPath { component })?;
		result.node = entry.node;
		result.kind = entry.kind;
		if parts.peek().is_none() {
			result.name = Some(Name::from_native(entry.name).ok_or(Error::UnsupportedName)?);
		}
	}
	Ok(result)
}

fn admit(path: &str) -> Result<&str, InvalidPath> {
	let tail = path.strip_prefix(SEPARATOR).ok_or(InvalidPath::Relative)?;
	if path.len() > MAX_PATH_BYTES {
		return Err(InvalidPath::TooLong {
			maximum_bytes: MAX_PATH_BYTES,
		});
	}
	if tail.is_empty() {
		return Ok(tail);
	}
	for (index, name) in tail.split(SEPARATOR).enumerate() {
		if index >= MAX_PATH_COMPONENTS {
			return Err(InvalidPath::TooManyComponents {
				maximum: MAX_PATH_COMPONENTS,
			});
		}
		let component = index + 1;
		if name.is_empty()
			|| matches!(name, CURRENT_DIRECTORY | PARENT_DIRECTORY)
			|| name.contains('\0')
		{
			return Err(InvalidPath::InvalidComponent { component });
		}
		if name.len() > MAX_COMPONENT_BYTES {
			return Err(InvalidPath::ComponentTooLong {
				component,
				maximum_bytes: MAX_COMPONENT_BYTES,
			});
		}
	}
	Ok(tail)
}
