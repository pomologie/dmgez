use std::{
	collections::{HashMap, HashSet},
	error::Error as StdError,
	num::NonZeroU32,
};

use dmgez_vfs::volume::{Metadata, NodeKind, StorageId};

use crate::Side;

use super::{Error, catalog::Catalog, path_matching::Pairs, require_kind};

pub(super) struct Topology<'catalog> {
	equivalent: HashSet<&'catalog StorageId>,
}

struct Group<'catalog> {
	count: NonZeroU32,
	paths: Vec<&'catalog [String]>,
}

impl<'catalog> Topology<'catalog> {
	pub(super) fn compare<L: StdError + 'static, R: StdError + 'static>(
		left: &'catalog Catalog<impl Copy>,
		right: &'catalog Catalog<impl Copy>,
		pairs: &Pairs<'catalog>,
		cancelled: &mut impl FnMut() -> bool,
	) -> Result<Self, Error<L, R>> {
		let left_links = groups(left, Side::Left, &mut *cancelled)?;
		let right_links = groups(right, Side::Right, &mut *cancelled)?;
		let right_storage = |path| {
			pairs
				.right(path)
				.and_then(|paired_path| right.get(paired_path))
				.and_then(|entry| entry.metadata.hard_link.as_ref())
				.map(|link| &link.storage)
		};
		let mut equivalent = HashSet::new();
		for (id, group) in left_links {
			if cancelled() {
				return Err(Error::Cancelled);
			}
			let Some(right_id) = group.paths.first().copied().and_then(right_storage) else {
				continue;
			};
			let Some(right_group) = right_links.get(right_id) else {
				continue;
			};
			if group.count != right_group.count || group.paths.len() != right_group.paths.len() {
				continue;
			}
			if matches!(pairs, Pairs::Exact) {
				if group.paths == right_group.paths {
					equivalent.insert(id);
				}
				continue;
			}
			// Pairing is one-to-one: if every left member maps to the same
			// right group of equal size, no additional right members remain.
			let mut same_group = true;
			for path in group.paths {
				if cancelled() {
					return Err(Error::Cancelled);
				}
				if right_storage(path) != Some(right_id) {
					same_group = false;
					break;
				}
			}
			if same_group {
				equivalent.insert(id);
			}
		}
		Ok(Self { equivalent })
	}

	pub(super) fn equivalent(
		&self,
		left: &Metadata<StorageId>,
		right: &Metadata<StorageId>,
	) -> bool {
		// A stored singleton hard-link record has ordinary one-name topology.
		match (
			left.hard_link.as_ref().filter(|link| link.count.get() > 1),
			right.hard_link.as_ref().filter(|link| link.count.get() > 1),
		) {
			(None, None) => true,
			(Some(left), Some(_)) => self.equivalent.contains(&left.storage),
			_ => false,
		}
	}
}

fn groups<'catalog, L: StdError + 'static, R: StdError + 'static>(
	catalog: &'catalog Catalog<impl Copy>,
	side: Side,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<HashMap<&'catalog StorageId, Group<'catalog>>, Error<L, R>> {
	let mut groups = HashMap::new();
	for (path, entry) in catalog {
		if cancelled() {
			return Err(Error::Cancelled);
		}
		require_kind(entry, side, path)?;
		let Some(link) = &entry.metadata.hard_link else {
			continue;
		};
		let group = groups.entry(&link.storage).or_insert_with(|| Group {
			count: link.count,
			paths: Vec::new(),
		});
		if entry.metadata.kind != NodeKind::RegularFile
			|| group.count != link.count
			|| group.paths.len() as u64 >= u64::from(link.count.get())
		{
			return Err(Error::LinkCount {
				side,
				path: path.clone(),
			});
		}
		group.paths.push(path);
	}
	Ok(groups)
}
