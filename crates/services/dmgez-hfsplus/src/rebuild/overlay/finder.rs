use std::{collections::BTreeMap, rc::Rc};

use dmgez_config::{VirtualPath, project_edit::FieldChange};
use dmgez_finder::{
	STORE_NAME,
	edit::{EditableStore, IconLocationRecord, StoreEditError},
	inspection::ReadLimits,
	write_store,
};
use hfsplus_builder::volume::EntryMetadata;
use hfsplus_core::names::normalize;

use super::super::{File, Kind, Node, Payload, capture::MAX_ENTRIES};
use super::{
	OverlayError,
	paths::{PATH_SEPARATOR, PathIndex, Route, TreeKind, charge, check_path_bounds},
};

const GENERATED_STORE_MODE: u16 = 0o644;

type Positions = BTreeMap<String, IconLocationRecord>;

pub(super) struct PositionEdit {
	index: usize,
	coordinates: Option<[u32; 2]>,
}

impl PositionEdit {
	pub(super) fn read(
		index: usize,
		change: FieldChange<[f64; 2]>,
	) -> Result<Option<Self>, OverlayError> {
		let coordinates = match change {
			FieldChange::Keep => return Ok(None),
			FieldChange::Clear => None,
			FieldChange::Set(point) => {
				if !point.iter().all(|&coordinate| {
					(0.0..=f64::from(u32::MAX)).contains(&coordinate) && coordinate.fract() == 0.0
				}) {
					return Err(OverlayError::FinderPosition { index });
				}
				Some(point.map(|coordinate| coordinate as u32))
			}
		};
		Ok(Some(Self { index, coordinates }))
	}
}

pub(super) struct LayoutPlan<'volume, S> {
	pub(super) additions: Vec<(VirtualPath, Node<'volume, S>)>,
	pub(super) replacements: BTreeMap<VirtualPath, Kind<'volume, S>>,
}

struct Location<'source> {
	// Retain an original spelling if its file name hasn't changed. The
	// source store may use a spelling equivalent under the HFS+ comparator.
	name: Option<&'source str>,
	record: IconLocationRecord,
}

pub(super) fn plan<'source, 'volume, S>(
	entries: &'source BTreeMap<VirtualPath, Node<'volume, S>>,
	routes: &[Route],
	original_paths: &PathIndex<'source, TreeKind>,
	final_paths: &PathIndex<'_, TreeKind>,
	positions: BTreeMap<&'source VirtualPath, PositionEdit>,
) -> Result<LayoutPlan<'volume, S>, OverlayError> {
	let mut retained = read_positions(entries, original_paths)?;
	if retained.is_empty() && positions.is_empty() {
		return Ok(LayoutPlan {
			additions: Vec::new(),
			replacements: BTreeMap::new(),
		});
	}
	let destinations: BTreeMap<_, _> = entries.keys().zip(routes).collect();
	for (path, edit) in positions {
		if destinations
			.get(path)
			.and_then(|route| route.path(path))
			.is_none()
		{
			return Err(OverlayError::ConflictingEdit { index: edit.index });
		}
		if let Some([x, y]) = edit.coordinates {
			retained
				.entry(path)
				.and_modify(|location| {
					location.record = location.record.with_position(x, y);
				})
				.or_insert_with(|| Location {
					name: None,
					record: IconLocationRecord::new(x, y),
				});
		} else {
			retained.remove(path);
		}
	}
	let mut by_folder = BTreeMap::<&str, Positions>::new();
	let mut retained_bytes = 0;
	for (original, location) in retained {
		let Some(destination) = destinations
			.get(original)
			.and_then(|route| route.path(original))
		else {
			continue;
		};
		let (parent, leaf) = components(destination)?;
		let folder = canonical_folder(final_paths, parent, destination)?;
		let name = match location.name {
			Some(name) if leaf == components(original)?.1 => name.to_owned(),
			_ => stored_name(destination, leaf)?,
		};
		charge(
			&mut retained_bytes,
			name.capacity() + size_of::<(String, IconLocationRecord)>(),
		)?;
		if by_folder
			.entry(folder)
			.or_default()
			.insert(name, location.record)
			.is_some()
		{
			return Err(OverlayError::PathProfile {
				path: destination.clone(),
			});
		}
	}

	let mut replacements = BTreeMap::new();
	for ((original, entry), route) in entries.iter().zip(routes) {
		let Kind::FinderStore { file, layout } = &entry.kind else {
			continue;
		};
		let Some(destination) = route.path(original) else {
			continue;
		};
		let (parent, _) = components(destination)?;
		let folder = canonical_folder(final_paths, parent, destination)?;
		let positions = by_folder.remove(folder).unwrap_or_default();
		if let Some(bytes) = layout
			.encode(&positions)
			.map_err(|source| layout_error(destination, source))?
		{
			let kind = prepared_store(
				destination,
				bytes,
				file.resource.clone(),
				&mut retained_bytes,
			)?;
			replacements.insert(original.clone(), kind);
		}
	}
	if by_folder.len() > MAX_ENTRIES.saturating_sub(final_paths.len()) {
		return Err(OverlayError::Budget);
	}
	let mut additions = Vec::new();
	additions.try_reserve_exact(by_folder.len())?;
	for (folder, positions) in by_folder {
		let path = child_path(folder, STORE_NAME).ok_or(OverlayError::Budget)?;
		check_path_bounds(&path)?;
		charge(&mut retained_bytes, path.as_str().len())?;
		if final_paths.find(&path)?.is_some() {
			return Err(OverlayError::FinderStore { path });
		}
		let default = write_store(None, &[]).map_err(|source| OverlayError::FinderDefaults {
			path: path.clone(),
			source,
		})?;
		let layout = EditableStore::open(&default, ReadLimits::default())
			.map_err(|source| layout_error(&path, source))?;
		let bytes = layout
			.encode(&positions)
			.map_err(|source| layout_error(&path, source))?
			.unwrap_or(default);
		let kind = prepared_store(&path, bytes, None, &mut retained_bytes)?;
		additions.push((
			path,
			Node {
				metadata: EntryMetadata::new(GENERATED_STORE_MODE),
				kind,
				attributes: Default::default(),
			},
		));
	}
	Ok(LayoutPlan {
		additions,
		replacements,
	})
}

fn read_positions<'source, S>(
	entries: &'source BTreeMap<VirtualPath, Node<'_, S>>,
	paths: &PathIndex<'source, TreeKind>,
) -> Result<BTreeMap<&'source VirtualPath, Location<'source>>, OverlayError> {
	let mut retained = BTreeMap::new();
	for (store, entry) in entries {
		let Kind::FinderStore { layout, .. } = &entry.kind else {
			continue;
		};
		let (parent, _) = components(store)?;
		for (name, record) in layout.positions() {
			let missing = || OverlayError::FinderRecordTarget {
				path: store.clone(),
				name: name.to_owned(),
			};
			let candidate = child_path(parent, name).ok_or_else(missing)?;
			let (owner, kind) = paths.find(&candidate)?.ok_or_else(missing)?;
			if *kind == TreeKind::FinderStore
				|| retained
					.insert(
						owner,
						Location {
							name: Some(name),
							record,
						},
					)
					.is_some()
			{
				return Err(missing());
			}
		}
	}
	Ok(retained)
}

fn components(path: &VirtualPath) -> Result<(&str, &str), OverlayError> {
	path.as_str()
		.rsplit_once(PATH_SEPARATOR)
		.filter(|(_, leaf)| !leaf.is_empty())
		.ok_or_else(|| OverlayError::PathProfile { path: path.clone() })
}

fn child_path(parent: &str, leaf: &str) -> Option<VirtualPath> {
	if leaf.contains(PATH_SEPARATOR) {
		return None;
	}
	format!("{parent}{PATH_SEPARATOR}{leaf}").parse().ok()
}

fn canonical_folder<'path>(
	paths: &PathIndex<'path, TreeKind>,
	parent: &str,
	child: &VirtualPath,
) -> Result<&'path str, OverlayError> {
	if parent.is_empty() {
		return Ok("");
	}
	let unavailable = || OverlayError::Parent {
		path: child.clone(),
	};
	let parent = parent.parse().map_err(|_| unavailable())?;
	match paths.find(&parent)? {
		Some((path, TreeKind::Directory)) => Ok(path.as_str()),
		_ => Err(unavailable()),
	}
}

fn stored_name(path: &VirtualPath, name: &str) -> Result<String, OverlayError> {
	let normalized = normalize(name).map_err(|source| OverlayError::PathName {
		path: path.clone(),
		source,
	})?;
	String::from_utf16(normalized.as_units())
		.map_err(|_| OverlayError::PathProfile { path: path.clone() })
}

fn prepared_store<'volume, S>(
	path: &VirtualPath,
	bytes: Vec<u8>,
	resource: Option<Payload<'volume, S>>,
	retained_bytes: &mut usize,
) -> Result<Kind<'volume, S>, OverlayError> {
	let layout = EditableStore::open(&bytes, ReadLimits::default())
		.map_err(|source| layout_error(path, source))?;
	charge(retained_bytes, bytes.capacity())?;
	charge(retained_bytes, layout.retained_bytes())?;
	Ok(Kind::FinderStore {
		file: Rc::new(File {
			data: Payload::bytes(bytes),
			resource,
		}),
		layout,
	})
}

fn layout_error(path: &VirtualPath, source: StoreEditError) -> OverlayError {
	OverlayError::FinderLayout {
		path: path.clone(),
		source,
	}
}
