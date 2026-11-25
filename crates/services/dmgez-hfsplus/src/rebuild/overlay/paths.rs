use std::collections::BTreeMap;

use dmgez_config::VirtualPath;
use dmgez_finder::STORE_NAME;
use hfsplus_core::names::{NameCollation, NameError, normalize};

use super::super::capture::{MAX_CAPTURE_BYTES, MAX_DEPTH, MAX_ENTRIES, MAX_PATH_BYTES};
use super::{Change, Kind, Node, OverlayError};

pub(super) const PATH_SEPARATOR: char = '/';
// Case-folded collation keys contain no zero weights.
const COMPONENT_BOUNDARY: u16 = 0;
const DOT_WEIGHT: u16 = '.' as u16;
pub(super) const COLLATION: NameCollation = NameCollation::CaseFolded;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum TreeKind {
	Directory,
	FinderStore,
	Content,
}

impl TreeKind {
	pub(super) fn from_kind<S>(kind: &Kind<'_, S>) -> Self {
		match kind {
			Kind::Directory => Self::Directory,
			Kind::FinderStore { .. } => Self::FinderStore,
			Kind::File { .. } | Kind::Symlink(_) => Self::Content,
		}
	}
}

pub(super) enum Route {
	Keep,
	Move(VirtualPath),
	Remove,
}

impl Route {
	pub(super) fn path<'route>(
		&'route self,
		original: &'route VirtualPath,
	) -> Option<&'route VirtualPath> {
		match self {
			Self::Keep => Some(original),
			Self::Move(path) => Some(path),
			Self::Remove => None,
		}
	}
}

pub(super) fn route_entries<S>(
	entries: &BTreeMap<VirtualPath, Node<'_, S>>,
	changes: &BTreeMap<&VirtualPath, Change<'_>>,
	mut path_bytes: usize,
) -> Result<Vec<Route>, OverlayError> {
	let mut routes = Vec::<Route>::new();
	routes.try_reserve_exact(entries.len())?;
	let mut directories = BTreeMap::<&str, (&VirtualPath, usize)>::new();
	for (path, entry) in entries {
		let (parent, leaf) = path
			.as_str()
			.rsplit_once(PATH_SEPARATOR)
			.ok_or_else(|| OverlayError::SourceHierarchy { path: path.clone() })?;
		let final_parent = if parent.is_empty() {
			Some(parent)
		} else {
			let &(original, route_index) = directories
				.get(parent)
				.ok_or_else(|| OverlayError::SourceHierarchy { path: path.clone() })?;
			let previous = routes
				.get(route_index)
				.ok_or_else(|| OverlayError::SourceHierarchy { path: path.clone() })?;
			previous.path(original).map(VirtualPath::as_str)
		};
		let route = match (changes.get(path), final_parent) {
			(Some(Change::Move { index, .. }), None) => {
				return Err(OverlayError::ConflictingEdit { index: *index });
			}
			(Some(Change::Remove) | None, None) | (Some(Change::Remove), Some(_)) => Route::Remove,
			(Some(Change::Move { destination, .. }), Some(_)) => {
				Route::Move((*destination).clone())
			}
			(None, Some(destination)) if destination == parent => Route::Keep,
			(None, Some(destination)) => {
				if destination
					.len()
					.saturating_add(1)
					.saturating_add(leaf.len())
					> MAX_PATH_BYTES
				{
					return Err(OverlayError::Budget);
				}
				let joined = format!("{destination}{PATH_SEPARATOR}{leaf}")
					.parse()
					.map_err(|_| OverlayError::PathProfile { path: path.clone() })?;
				Route::Move(joined)
			}
		};
		if let Some(path) = route.path(path) {
			check_path_bounds(path)?;
			charge(&mut path_bytes, path.as_str().len())?;
		}
		if matches!(entry.kind, Kind::Directory) {
			directories.insert(path.as_str(), (path, routes.len()));
		}
		routes.push(route);
	}
	Ok(routes)
}

pub(super) fn validate_tree<'a>(
	entries: impl Iterator<Item = (&'a VirtualPath, TreeKind)>,
) -> Result<PathIndex<'a, TreeKind>, OverlayError> {
	let mut index = PathIndex::default();
	for (path, kind) in entries {
		index.insert(path, kind)?;
	}
	let store_name = component_key(STORE_NAME).map_err(|_| OverlayError::VolumeName)?;
	for (key, &(path, kind)) in &index.paths {
		if key.rsplit(|unit| *unit == COMPONENT_BOUNDARY).next() == Some(store_name.as_slice())
			&& kind != TreeKind::FinderStore
		{
			return Err(OverlayError::FinderStore { path: path.clone() });
		}
		if let Some(boundary) = key.iter().rposition(|unit| *unit == COMPONENT_BOUNDARY)
			&& !index
				.paths
				.get(&key[..boundary])
				.is_some_and(|(_, kind)| *kind == TreeKind::Directory)
		{
			return Err(OverlayError::Parent { path: path.clone() });
		}
	}
	Ok(index)
}

pub(super) struct PathIndex<'a, T> {
	paths: BTreeMap<Vec<u16>, (&'a VirtualPath, T)>,
	key_bytes: usize,
}

impl<T> Default for PathIndex<'_, T> {
	fn default() -> Self {
		Self {
			paths: BTreeMap::new(),
			key_bytes: 0,
		}
	}
}

impl<'a, T> PathIndex<'a, T> {
	pub(super) fn insert(&mut self, path: &'a VirtualPath, value: T) -> Result<(), OverlayError> {
		if self.paths.len() == MAX_ENTRIES {
			return Err(OverlayError::Budget);
		}
		let key = path_key(path)?;
		charge(&mut self.key_bytes, std::mem::size_of_val(key.as_slice()))?;
		if let Some((first, _)) = self.paths.insert(key, (path, value)) {
			return Err(OverlayError::Collision {
				first: first.clone(),
				second: path.clone(),
			});
		}
		Ok(())
	}

	pub(super) fn find(
		&self,
		path: &VirtualPath,
	) -> Result<Option<(&'a VirtualPath, &T)>, OverlayError> {
		let key = path_key(path)?;
		Ok(self.paths.get(&key).map(|(path, value)| (*path, value)))
	}

	pub(super) fn len(&self) -> usize {
		self.paths.len()
	}
}

fn path_key(path: &VirtualPath) -> Result<Vec<u16>, OverlayError> {
	check_path_bounds(path)?;
	let mut key = Vec::new();
	for component in path.components() {
		let name = component_key(component).map_err(|source| OverlayError::PathName {
			path: path.clone(),
			source,
		})?;
		if matches!(
			name.as_slice(),
			[] | [DOT_WEIGHT] | [DOT_WEIGHT, DOT_WEIGHT]
		) {
			return Err(OverlayError::PathProfile { path: path.clone() });
		}
		let separated = !key.is_empty();
		key.try_reserve(name.len() + usize::from(separated))?;
		if separated {
			key.push(COMPONENT_BOUNDARY);
		}
		key.extend_from_slice(&name);
	}
	Ok(key)
}

pub(super) fn check_path_bounds(path: &VirtualPath) -> Result<(), OverlayError> {
	if path.as_str().len() > MAX_PATH_BYTES || path.components().count() > MAX_DEPTH {
		return Err(OverlayError::Budget);
	}
	Ok(())
}

pub(super) fn charge(total: &mut usize, bytes: usize) -> Result<(), OverlayError> {
	*total = total
		.checked_add(bytes)
		.filter(|count| *count <= MAX_CAPTURE_BYTES)
		.ok_or(OverlayError::Budget)?;
	Ok(())
}

fn component_key(component: &str) -> Result<Vec<u16>, NameError> {
	COLLATION.key(normalize(component)?.as_units())
}
