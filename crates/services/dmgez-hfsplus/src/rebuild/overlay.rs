use std::{
	collections::BTreeMap, collections::TryReserveError, error::Error as StdError, path::Path,
};

use dmgez_config::{
	VirtualPath,
	project_edit::{ContentDraft, ProjectEdit},
};
use dmgez_finder::edit::StoreEditError;
use hfsplus_builder::volume::{EntryMetadata, MAX_SYMLINK_TARGET_BYTES};
use hfsplus_core::{
	ReadAt,
	names::{NameError, normalize},
};
use thiserror::Error;

use super::{Kind, Node, Snapshot, background::Bindings, capture::MAX_ENTRIES};

mod finder;
mod finder_flags;
mod paths;
#[cfg(test)]
mod tests;

use finder_flags::{FlagEdits, FlagPlan};
use paths::{
	COLLATION, PATH_SEPARATOR, PathIndex, Route, TreeKind, charge, check_path_bounds,
	route_entries, validate_tree,
};

const DIRECTORY_MODE: u16 = 0o755;
const SYMLINK_MODE: u16 = 0o777;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OverlayError {
	#[error("image edit {index} targets a path absent from the original image: {path:?}")]
	UnknownTarget { index: usize, path: VirtualPath },
	#[error("image edit {index} conflicts with removal of the target or its ancestor")]
	ConflictingEdit { index: usize },
	#[error("image edit {index} selects the volume root as an entry destination")]
	RootDestination { index: usize },
	#[error(
		"Finder position in image edit {index} requires nonnegative whole points in the u32 range"
	)]
	FinderPosition { index: usize },
	#[error("Finder flag edit would change signing-sensitive metadata: {path:?}")]
	FinderSignedTarget { path: VirtualPath },
	#[error("could not inspect the Finder flag edit target {path:?} for code signing: {reason}")]
	FinderSigningProbe { path: VirtualPath, reason: String },
	#[error("image edit {index} would copy host content without a qualified metadata capture")]
	HostCopy { index: usize },
	#[error("the captured host copies do not belong to this edit batch")]
	UnexpectedHostCopies,
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
	#[error("Finder layout path is reserved: {path:?}")]
	FinderStore { path: VirtualPath },
	#[error("Finder store {path:?} cannot resolve an unambiguous catalog child for {name:?}")]
	FinderRecordTarget { path: VirtualPath, name: String },
	#[error("Finder background target would not survive the edit: {target:?} for {store:?}")]
	FinderBackgroundRemoved {
		store: VirtualPath,
		target: VirtualPath,
	},
	#[error(
		"Finder background target or parent is missing from the rebuilt catalog: {target:?} for {store:?}"
	)]
	FinderBackgroundTarget {
		store: VirtualPath,
		target: VirtualPath,
	},
	#[error("Finder store {path:?} cannot represent the edited layout")]
	FinderLayout {
		path: VirtualPath,
		#[source]
		source: StoreEditError,
	},
	#[error("could not create default Finder settings for {path:?}")]
	FinderDefaults {
		path: VirtualPath,
		#[source]
		source: dmgez_finder::Error,
	},
}

enum Change<'edit> {
	Move {
		index: usize,
		destination: &'edit VirtualPath,
	},
	Remove,
}

/// Apply atomically with respect to the source snapshot. Edits select the original
/// catalog spelling by native name equivalence. Explicit moves name their final
/// absolute destinations. Otherwise entries inherit moves and removals of their
/// parent directories.
pub(super) fn apply<S: ReadAt>(
	snapshot: &mut Snapshot<'_, S>,
	edits: &[ProjectEdit],
	_changes_dir: &Path,
) -> Result<Bindings, OverlayError>
where
	S::Error: StdError + 'static,
{
	apply_with_copies(snapshot, edits, Vec::new())
}

/// The caller supplies only entries captured for the copy edits in this batch.
/// All destinations are validated before the snapshot is modified.
pub(super) fn apply_with_copies<'volume, S: ReadAt>(
	snapshot: &mut Snapshot<'volume, S>,
	edits: &[ProjectEdit],
	copies: Vec<(VirtualPath, Node<'volume, S>)>,
) -> Result<Bindings, OverlayError>
where
	S::Error: StdError + 'static,
{
	if copies.is_empty()
		&& edits.is_empty()
		&& !snapshot
			.entries
			.values()
			.any(|entry| matches!(entry.kind, Kind::FinderStore { .. }))
	{
		return Ok(Bindings::new());
	}
	let mut plan = plan(snapshot, edits, copies)?;
	let mut entries = BTreeMap::new();
	for (index, ((original, mut entry), route)) in std::mem::take(&mut snapshot.entries)
		.into_iter()
		.zip(plan.routes)
		.enumerate()
	{
		if let Some(kind) = plan.replacements.remove(&original) {
			entry.kind = kind;
		}
		if let Some(info) = plan.finder_flags.remove(&index) {
			entry.metadata.finder_info = info;
		}
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
	Ok(plan.backgrounds)
}

struct Plan<'edit, 'v, S> {
	routes: Vec<Route>,
	additions: Vec<(VirtualPath, Node<'v, S>)>,
	replacements: BTreeMap<VirtualPath, Kind<'v, S>>,
	finder_flags: FlagPlan,
	backgrounds: Bindings,
	label: Option<&'edit str>,
}

fn plan<'edit, 'v, S: ReadAt>(
	snapshot: &Snapshot<'v, S>,
	edits: &'edit [ProjectEdit],
	mut additions: Vec<(VirtualPath, Node<'v, S>)>,
) -> Result<Plan<'edit, 'v, S>, OverlayError>
where
	S::Error: StdError + 'static,
{
	let entries = &snapshot.entries;
	let captured_count = additions.len();
	if captured_count > MAX_ENTRIES {
		return Err(OverlayError::Budget);
	}
	let mut added_path_bytes = 0;
	for (path, node) in &additions {
		if path.is_root() {
			return Err(OverlayError::PathProfile { path: path.clone() });
		}
		if matches!(node.kind, Kind::FinderStore { .. }) {
			return Err(OverlayError::FinderStore { path: path.clone() });
		}
		check_path_bounds(path)?;
		charge(&mut added_path_bytes, path.as_str().len())?;
	}
	let mut copy_edits = 0;
	let mut changes = BTreeMap::new();
	let mut positions = BTreeMap::new();
	let mut finder_flags = FlagEdits::default();
	let mut label = None;
	let mut index = PathIndex::default();
	for (path, entry) in entries {
		index.insert(path, TreeKind::from_kind(&entry.kind))?;
	}
	{
		for (edit_index, edit) in edits.iter().enumerate() {
			match edit {
				ProjectEdit::Add(ContentDraft::Copy { .. }) => {
					if copy_edits == captured_count {
						return Err(OverlayError::HostCopy { index: edit_index });
					}
					copy_edits += 1;
				}
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
					let target = ordinary_target(&index, edit_index, target)?;
					if matches!(changes.get(target), Some(Change::Remove)) {
						return Err(OverlayError::ConflictingEdit { index: edit_index });
					}
					finder_flags.record(target, &entries[target], edit_index, fields);
					if let Some(change) = finder::PositionEdit::read(edit_index, fields.position)? {
						positions.insert(target, change);
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
					let target = ordinary_target(&index, edit_index, target)?;
					if positions.contains_key(target)
						|| finder_flags.targets(target)
						|| changes.insert(target, Change::Remove).is_some()
					{
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
	if copy_edits == 0 && captured_count != 0 {
		return Err(OverlayError::UnexpectedHostCopies);
	}
	let routes = route_entries(entries, &changes, added_path_bytes)?;
	let finder_flags = finder_flags.finish(entries, &routes, &additions)?;
	let layout = {
		let final_paths = validate_tree(
			entries
				.iter()
				.zip(&routes)
				.filter_map(|((original, entry), route)| {
					route
						.path(original)
						.map(|path| (path, TreeKind::from_kind(&entry.kind)))
				})
				.chain(
					additions
						.iter()
						.map(|(path, entry)| (path, TreeKind::from_kind(&entry.kind))),
				),
		)?;
		finder::plan(snapshot, &routes, &index, &final_paths, positions)?
	};
	additions.try_reserve(layout.additions.len())?;
	additions.extend(layout.additions);
	Ok(Plan {
		routes,
		additions,
		replacements: layout.replacements,
		finder_flags,
		backgrounds: layout.backgrounds,
		label,
	})
}

fn ordinary_target<'path>(
	index: &PathIndex<'path, TreeKind>,
	edit: usize,
	target: &VirtualPath,
) -> Result<&'path VirtualPath, OverlayError> {
	match index.find(target)? {
		Some((path, TreeKind::FinderStore)) => {
			Err(OverlayError::FinderStore { path: path.clone() })
		}
		Some((path, _)) => Ok(path),
		None => Err(OverlayError::UnknownTarget {
			index: edit,
			path: target.clone(),
		}),
	}
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
