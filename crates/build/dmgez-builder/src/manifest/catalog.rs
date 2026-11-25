use std::{
	collections::{BTreeMap, btree_map::Entry as MapEntry},
	mem::size_of,
	path::PathBuf,
};

use dmgez_config::Finder;

use super::{
	Budget, Entry, EntryKind, Error, FinderOverrides, Limits, Manifest, PATH_SEPARATOR,
	SourceSnapshot, VirtualPath,
};

/// A Finder flag selected by a project-wide path list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum VisibilityFlag {
	/// Mark the entry invisible in Finder.
	Hidden,
	/// Hide the entry's name extension in Finder.
	HideExtension,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Origin {
	Synthesized,
	DeclaredDirectory,
	#[cfg(any(unix, windows))]
	Host,
	#[cfg(any(unix, windows))]
	MergedDirectory,
	Other,
}

#[derive(Debug)]
pub(super) struct Body {
	pub(super) kind: EntryKind,
	pub(super) source: Option<SourceSnapshot>,
	pub(super) finder: FinderOverrides,
	pub(super) origin: Origin,
}

impl Body {
	pub(super) fn generated(kind: EntryKind, finder: FinderOverrides, origin: Origin) -> Self {
		Self {
			kind,
			source: None,
			finder,
			origin,
		}
	}

	fn bytes(&self) -> Option<usize> {
		let source = self
			.source
			.as_ref()
			.map_or(0, |node| node.path.as_os_str().len());
		let target = match &self.kind {
			EntryKind::Symlink(bytes) => bytes.len(),
			_ => 0,
		};
		let icon = self
			.finder
			.custom_icon
			.as_ref()
			.map_or(0, |icon| icon.as_path().as_os_str().len());
		source.checked_add(target)?.checked_add(icon)
	}

	fn file_bytes(&self) -> u64 {
		if matches!(self.kind, EntryKind::File) {
			self.source.as_ref().map_or(0, |node| node.metadata.size)
		} else {
			0
		}
	}
}

pub(super) struct Catalog {
	entries: BTreeMap<VirtualPath, Body>,
	pub(super) limits: Limits,
	count: u64,
	regular_payload_bytes: u64,
	metadata_bytes: u64,
}

impl Catalog {
	pub(super) fn new(limits: Limits) -> Self {
		Self {
			entries: BTreeMap::new(),
			limits,
			count: 0,
			regular_payload_bytes: 0,
			metadata_bytes: 0,
		}
	}

	pub(super) fn validate_destination(&self, destination: &VirtualPath) -> Result<(), Error> {
		if destination.is_root() {
			return Err(Error::RootDestination);
		}
		if destination.components().count() > self.limits.max_depth {
			return Err(Error::Limit(Budget::Depth));
		}
		Ok(())
	}

	pub(super) fn admit(&mut self, destination: VirtualPath, body: Body) -> Result<(), Error> {
		self.validate_destination(&destination)?;
		for (end, _) in destination.as_str().rmatch_indices(PATH_SEPARATOR) {
			if end == 0 {
				break;
			}
			let parent = destination
				.as_str()
				.get(..end)
				.ok_or(Error::RootDestination)?;
			let path = parse_parent(parent)?;
			match self.entries.get(&path) {
				Some(entry) if !matches!(entry.kind, EntryKind::Directory) => {
					return Err(Error::ParentConflict { path });
				}
				Some(_) => break,
				None => {
					self.insert(
						path,
						Body::generated(
							EntryKind::Directory,
							FinderOverrides::default(),
							Origin::Synthesized,
						),
					)?;
				}
			}
		}
		self.insert(destination, body)
	}

	fn insert(&mut self, destination: VirtualPath, body: Body) -> Result<(), Error> {
		#[cfg(any(unix, windows))]
		let mut body = body;
		let limits = self.limits;
		let (existing_bytes, count) = if let Some(existing) = self.entries.get(&destination) {
			if existing.origin == Origin::Synthesized && !matches!(body.kind, EntryKind::Directory)
			{
				return Err(Error::ParentConflict { path: destination });
			}
			if !matches!(body.kind, EntryKind::Directory)
				|| !matches!(existing.kind, EntryKind::Directory)
			{
				return Err(Error::DestinationConflict { path: destination });
			}
			if existing.origin != Origin::Synthesized {
				#[cfg(any(unix, windows))]
				merge_directory(&destination, existing, &mut body)?;
				#[cfg(not(any(unix, windows)))]
				return Err(Error::DestinationConflict { path: destination });
			}
			(
				existing.bytes().ok_or(Error::Limit(Budget::Metadata))?,
				self.count,
			)
		} else {
			let count = self
				.count
				.checked_add(1)
				.ok_or(Error::Limit(Budget::Entries))?;
			if count > limits.max_entries {
				return Err(Error::Limit(Budget::Entries));
			}
			(0, count)
		};

		let entry = self.entries.entry(destination);
		let body_bytes = body.bytes().ok_or(Error::Limit(Budget::Metadata))?;
		let extra = match &entry {
			MapEntry::Vacant(entry) => size_of::<Entry>()
				.checked_add(entry.key().as_str().len())
				.ok_or(Error::Limit(Budget::Metadata))?,
			MapEntry::Occupied(_) => 0,
		};
		let additional = u64::try_from(
			body_bytes
				.checked_add(extra)
				.ok_or(Error::Limit(Budget::Metadata))?,
		)
		.map_err(|_| Error::Limit(Budget::Metadata))?;
		let removed = u64::try_from(existing_bytes).map_err(|_| Error::Limit(Budget::Metadata))?;
		let bytes = self
			.metadata_bytes
			.checked_sub(removed)
			.and_then(|bytes| bytes.checked_add(additional))
			.filter(|&bytes| bytes <= limits.max_metadata_bytes)
			.ok_or(Error::Limit(Budget::Metadata))?;
		let payload = self
			.regular_payload_bytes
			.checked_add(body.file_bytes())
			.ok_or(Error::Limit(Budget::Payload))?;
		match entry {
			MapEntry::Vacant(entry) => {
				entry.insert(body);
			}
			MapEntry::Occupied(mut entry) => {
				entry.insert(body);
			}
		}
		self.metadata_bytes = bytes;
		self.regular_payload_bytes = payload;
		self.count = count;
		Ok(())
	}

	#[cfg(any(unix, windows))]
	pub(super) const fn metadata_bytes(&self) -> u64 {
		self.metadata_bytes
	}

	pub(super) fn finish(self) -> Result<Manifest, Error> {
		let mut entries = Vec::new();
		entries.try_reserve_exact(self.entries.len())?;
		entries.extend(self.entries.into_iter().map(|(destination, body)| Entry {
			destination,
			kind: body.kind,
			source: body.source,
			finder: body.finder,
		}));
		Ok(Manifest {
			entries,
			auxiliary_sources: Vec::new(),
			regular_payload_bytes: self.regular_payload_bytes,
			metadata_bytes: self.metadata_bytes,
		})
	}

	pub(super) fn select_visibility(
		&mut self,
		finder: &Finder,
		cancelled: &mut impl FnMut() -> bool,
	) -> Result<(), Error> {
		for (flag, selection) in [
			(VisibilityFlag::Hidden, &finder.hide),
			(VisibilityFlag::HideExtension, &finder.hide_extensions),
		] {
			for path in selection {
				if cancelled() {
					return Err(Error::Cancelled);
				}
				let entry =
					self.entries
						.get_mut(path)
						.ok_or_else(|| Error::FinderTargetMissing {
							path: path.clone(),
							flag,
						})?;
				let value = match flag {
					VisibilityFlag::Hidden => &mut entry.finder.hidden,
					VisibilityFlag::HideExtension => &mut entry.finder.hide_extension,
				};
				if *value == Some(false) {
					return Err(Error::FinderTargetConflict {
						path: path.clone(),
						flag,
					});
				}
				*value = Some(true);
			}
		}
		Ok(())
	}
}

fn parse_parent(parent: &str) -> Result<VirtualPath, Error> {
	parent.parse().map_err(|_| Error::InvalidSourceName {
		path: PathBuf::from(parent),
	})
}

#[cfg(any(unix, windows))]
fn merge_directory(
	destination: &VirtualPath,
	existing: &Body,
	body: &mut Body,
) -> Result<(), Error> {
	let conflict = || Error::DestinationConflict {
		path: destination.clone(),
	};
	match (existing.origin, body.origin) {
		(Origin::DeclaredDirectory, Origin::Host) => {}
		(Origin::Host, Origin::DeclaredDirectory) => {
			body.source = existing.source.clone();
		}
		_ => return Err(conflict()),
	}
	let old = &existing.finder;
	let new = &mut body.finder;
	if !merge_override(&old.position, &mut new.position)
		|| !merge_override(&old.hidden, &mut new.hidden)
		|| !merge_override(&old.hide_extension, &mut new.hide_extension)
		|| !merge_override(&old.custom_icon, &mut new.custom_icon)
	{
		return Err(conflict());
	}
	body.origin = Origin::MergedDirectory;
	Ok(())
}

#[cfg(any(unix, windows))]
fn merge_override<T: Clone + PartialEq>(old: &Option<T>, new: &mut Option<T>) -> bool {
	match (old, new.as_ref()) {
		(Some(old), Some(new)) => old == new,
		(Some(old), None) => {
			*new = Some(old.clone());
			true
		}
		(None, _) => true,
	}
}
