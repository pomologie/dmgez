use std::{collections::BTreeMap, collections::TryReserveError, path::Path};

use dmgez_config::{
	VirtualPath,
	project_edit::{ContentDraft, FieldChange, ProjectEdit},
};
use hfsplus_builder::volume::{EntryMetadata, MAX_SYMLINK_TARGET_BYTES};
use hfsplus_core::{
	ReadAt,
	names::{NameCollation, NameError, normalize},
};
use thiserror::Error;

use super::{
	Kind, Node, Snapshot,
	capture::{MAX_CAPTURE_BYTES, MAX_DEPTH, MAX_ENTRIES, MAX_PATH_BYTES},
};

#[cfg(test)]
mod tests;

const PATH_SEPARATOR: char = '/';
// Case-folded collation keys contain no zero weights.
const COMPONENT_BOUNDARY: u16 = 0;
const DOT_WEIGHT: u16 = '.' as u16;
const COLLATION: NameCollation = NameCollation::CaseFolded;
const DIRECTORY_MODE: u16 = 0o755;
const SYMLINK_MODE: u16 = 0o777;
const FINDER_STORE_NAME: &str = ".DS_Store";

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OverlayError {
	#[error("image edit {index} targets a path absent from the original image: {path:?}")]
	UnknownTarget { index: usize, path: VirtualPath },
	#[error("image edit {index} conflicts with removal of the target or its ancestor")]
	ConflictingEdit { index: usize },
	#[error("image edit {index} selects the volume root as an entry destination")]
	RootDestination { index: usize },
	#[error("Finder position and flag changes are unsupported for existing images (edit {index})")]
	FinderChange { index: usize },
	#[error("image edit {index} would copy host content without a qualified metadata capture")]
	HostCopy { index: usize },
	#[error("image edit {index} uses an operation unsupported for existing images")]
	UnsupportedEdit { index: usize },
	#[error("image edit {index} supplies an empty, NUL-containing, or oversized symlink target")]
	SymlinkTarget { index: usize },
	#[error("the edited volume name is outside the HFS+ name profile")]
	VolumeName,
	#[error("could not represent {path:?} as an HFS+ catalog path")]
	PathName {
		path: VirtualPath,
		#[source]
		source: NameError,
	},
	#[error("image path is outside the supported rebuild profile: {path:?}")]
	PathProfile { path: VirtualPath },
	#[error("the overlay exceeds its path or entry budget")]
	Budget,
	#[error("could not reserve bounded overlay memory")]
	Allocation(#[from] TryReserveError),
	#[error("the original catalog is missing a directory parent for {path:?}")]
	SourceHierarchy { path: VirtualPath },
	#[error("the edited tree is missing a directory parent for {path:?}")]
	Parent { path: VirtualPath },
	#[error("edited paths {first:?} and {second:?} collide under HFS+ name matching")]
	Collision {
		first: VirtualPath,
		second: VirtualPath,
	},
	#[error("image rebuild cannot preserve the opaque Finder layout at {path:?}")]
	FinderStore { path: VirtualPath },
}

enum Change<'edit> {
	Move {
		index: usize,
		destination: &'edit VirtualPath,
	},
	Remove,
}

enum Route {
	Keep,
	Move(VirtualPath),
	Remove,
}

impl Route {
	fn path<'route>(&'route self, original: &'route VirtualPath) -> Option<&'route VirtualPath> {
		match self {
			Self::Keep => Some(original),
			Self::Move(path) => Some(path),
			Self::Remove => None,
		}
	}
}

/// Apply atomically with respect to the source snapshot. Edits select the original
/// catalog spelling by native name equivalence. Explicit moves name their final
/// absolute destinations. Otherwise entries inherit moves and removals of their
/// parent directories.
pub(super) fn apply<S: ReadAt>(
	snapshot: &mut Snapshot<'_, S>,
	edits: &[ProjectEdit],
	_changes_dir: &Path,
) -> Result<(), OverlayError> {
	if edits.is_empty() {
		return Ok(());
	}
	let plan = plan(&snapshot.entries, edits)?;
	let mut entries = BTreeMap::new();
	for ((original, entry), route) in std::mem::take(&mut snapshot.entries)
		.into_iter()
		.zip(plan.routes)
	{
		match route {
			Route::Keep => entries.insert(original, entry),
			Route::Move(destination) => entries.insert(destination, entry),
			Route::Remove => None,
		};
	}
	entries.extend(plan.additions);
	snapshot.entries = entries;
	if let Some(label) = plan.label {
		snapshot.name = label.to_owned();
	}
	Ok(())
}

struct Plan<'edit, 'v, S> {
	routes: Vec<Route>,
	additions: Vec<(VirtualPath, Node<'v, S>)>,
	label: Option<&'edit str>,
}

fn plan<'edit, 'v, S>(
	entries: &BTreeMap<VirtualPath, Node<'v, S>>,
	edits: &'edit [ProjectEdit],
) -> Result<Plan<'edit, 'v, S>, OverlayError> {
	let mut additions = Vec::new();
	let mut added_path_bytes = 0;
	let mut changes = BTreeMap::new();
	let mut label = None;
	{
		let mut index = PathIndex::default();
		for path in entries.keys() {
			index.insert(path, ())?;
		}
		for (edit_index, edit) in edits.iter().enumerate() {
			match edit {
				ProjectEdit::Add(draft) => {
					if additions.len() == MAX_ENTRIES {
						return Err(OverlayError::Budget);
					}
					let addition = add(edit_index, draft, &mut added_path_bytes)?;
					additions.try_reserve(1)?;
					additions.push(addition);
				}
				ProjectEdit::Modify {
					target,
					changes: fields,
				} => {
					let target = index.source(edit_index, target)?;
					if fields.position != FieldChange::Keep
						|| fields.hidden != FieldChange::Keep
						|| fields.hide_extension != FieldChange::Keep
					{
						return Err(OverlayError::FinderChange { index: edit_index });
					}
					if matches!(changes.get(target), Some(Change::Remove)) {
						return Err(OverlayError::ConflictingEdit { index: edit_index });
					}
					if let Some(destination) = &fields.destination {
						check_destination(edit_index, destination)?;
						changes.insert(
							target,
							Change::Move {
								index: edit_index,
								destination,
							},
						);
					}
				}
				ProjectEdit::Remove { target } => {
					let target = index.source(edit_index, target)?;
					if changes.insert(target, Change::Remove).is_some() {
						return Err(OverlayError::ConflictingEdit { index: edit_index });
					}
				}
				ProjectEdit::SetImageName(name) => {
					check_label(name)?;
					label = Some(name.as_str());
				}
				_ => return Err(OverlayError::UnsupportedEdit { index: edit_index }),
			}
		}
	}
	let routes = route_entries(entries, &changes, added_path_bytes)?;
	validate_tree(
		entries
			.iter()
			.zip(&routes)
			.filter_map(|((original, entry), route)| {
				route.path(original).map(|path| (path, is_directory(entry)))
			})
			.chain(
				additions
					.iter()
					.map(|(path, entry)| (path, is_directory(entry))),
			),
	)?;
	Ok(Plan {
		routes,
		additions,
		label,
	})
}

fn add<'v, S>(
	index: usize,
	draft: &ContentDraft,
	path_bytes: &mut usize,
) -> Result<(VirtualPath, Node<'v, S>), OverlayError> {
	let (destination, mode, kind) = match draft {
		ContentDraft::Directory { destination } => (destination, DIRECTORY_MODE, Kind::Directory),
		ContentDraft::Symlink {
			destination,
			target,
		} => {
			let bytes = target.as_bytes();
			if bytes.is_empty() || bytes.len() > MAX_SYMLINK_TARGET_BYTES || bytes.contains(&0) {
				return Err(OverlayError::SymlinkTarget { index });
			}
			(destination, SYMLINK_MODE, Kind::Symlink(bytes.to_vec()))
		}
		ContentDraft::Copy { .. } => return Err(OverlayError::HostCopy { index }),
		_ => return Err(OverlayError::UnsupportedEdit { index }),
	};
	check_destination(index, destination)?;
	charge(path_bytes, destination.as_str().len())?;
	Ok((
		destination.clone(),
		Node {
			metadata: EntryMetadata::new(mode),
			kind,
			attributes: Default::default(),
		},
	))
}

fn check_destination(index: usize, destination: &VirtualPath) -> Result<(), OverlayError> {
	if destination.is_root() {
		return Err(OverlayError::RootDestination { index });
	}
	check_path_bounds(destination)
}

fn check_label(label: &str) -> Result<(), OverlayError> {
	if label.contains([PATH_SEPARATOR, ':', '\0']) {
		return Err(OverlayError::VolumeName);
	}
	let name = normalize(label).map_err(|_| OverlayError::VolumeName)?;
	if COLLATION.compare(name.as_units(), &[]).is_eq() {
		return Err(OverlayError::VolumeName);
	}
	Ok(())
}

fn route_entries<S>(
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
		if is_directory(entry) {
			directories.insert(path.as_str(), (path, routes.len()));
		}
		routes.push(route);
	}
	Ok(routes)
}

fn is_directory<S>(entry: &Node<'_, S>) -> bool {
	matches!(entry.kind, Kind::Directory)
}

fn validate_tree<'a>(
	entries: impl Iterator<Item = (&'a VirtualPath, bool)>,
) -> Result<(), OverlayError> {
	let mut index = PathIndex::default();
	for (path, directory) in entries {
		index.insert(path, directory)?;
	}
	let store_name = component_key(FINDER_STORE_NAME).map_err(|_| OverlayError::VolumeName)?;
	for (key, &(path, _)) in &index.paths {
		if key.rsplit(|unit| *unit == COMPONENT_BOUNDARY).next() == Some(store_name.as_slice()) {
			return Err(OverlayError::FinderStore { path: path.clone() });
		}
		if let Some(boundary) = key.iter().rposition(|unit| *unit == COMPONENT_BOUNDARY)
			&& !index
				.paths
				.get(&key[..boundary])
				.is_some_and(|(_, directory)| *directory)
		{
			return Err(OverlayError::Parent { path: path.clone() });
		}
	}
	Ok(())
}

#[derive(Default)]
struct PathIndex<'a, T> {
	paths: BTreeMap<Vec<u16>, (&'a VirtualPath, T)>,
	key_bytes: usize,
}

impl<'a, T> PathIndex<'a, T> {
	fn insert(&mut self, path: &'a VirtualPath, value: T) -> Result<(), OverlayError> {
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

	fn source(&self, index: usize, target: &VirtualPath) -> Result<&'a VirtualPath, OverlayError> {
		let key = path_key(target)?;
		self.paths
			.get(&key)
			.map(|(path, _)| *path)
			.ok_or_else(|| OverlayError::UnknownTarget {
				index,
				path: target.clone(),
			})
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

fn check_path_bounds(path: &VirtualPath) -> Result<(), OverlayError> {
	if path.as_str().len() > MAX_PATH_BYTES || path.components().count() > MAX_DEPTH {
		return Err(OverlayError::Budget);
	}
	Ok(())
}

fn charge(total: &mut usize, bytes: usize) -> Result<(), OverlayError> {
	*total = total
		.checked_add(bytes)
		.filter(|count| *count <= MAX_CAPTURE_BYTES)
		.ok_or(OverlayError::Budget)?;
	Ok(())
}

fn component_key(component: &str) -> Result<Vec<u16>, NameError> {
	COLLATION.key(normalize(component)?.as_units())
}
