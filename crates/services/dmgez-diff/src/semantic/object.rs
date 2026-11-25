#[cfg(feature = "content-hash")]
use std::num::NonZeroU32;

use dmgez_vfs::volume::{ForkKind, NodeKind, VolumeBackend, VolumeView};
#[cfg(feature = "content-hash")]
use dmgez_vfs::volume::{Metadata, StorageId};

use super::{Error, Field, catalog::Entry, links::Topology, payloads::Payloads};
#[cfg(feature = "finder-layout")]
use super::{FinderLayoutPolicy, FinderRecordChange, finder_layout};

#[derive(Clone, Copy)]
pub(super) struct PairedPaths<'a> {
	pub(super) left: &'a [String],
	pub(super) right: &'a [String],
}

#[derive(Default)]
pub(super) struct EntryChanges {
	pub(super) fields: Vec<Field>,
	#[cfg(feature = "finder-layout")]
	pub(super) finder_layout: Vec<FinderRecordChange>,
}

pub(super) fn compare_entry<L: VolumeBackend, R: VolumeBackend>(
	left: (&mut VolumeView<L>, &Entry<L::Id>),
	right: (&mut VolumeView<R>, &Entry<R::Id>),
	paths: PairedPaths<'_>,
	topology: &Topology<'_>,
	payloads: &mut Payloads<impl FnMut() -> bool>,
	#[cfg(feature = "finder-layout")] finder_layout: Option<FinderLayoutPolicy>,
) -> Result<EntryChanges, Error<L::Error, R::Error>> {
	let (left_view, left) = left;
	let (right_view, right) = right;
	let l = &left.metadata;
	let r = &right.metadata;
	let mut changes = EntryChanges::default();
	for (same, field) in [
		(l.kind == r.kind, Field::Kind),
		(l.mode == r.mode, Field::Mode),
		(l.uid == r.uid, Field::Uid),
		(l.gid == r.gid, Field::Gid),
		(topology.equivalent(l, r), Field::HardLinkTopology),
	] {
		if !same {
			changes.fields.push(field);
		}
	}
	match (l.kind, r.kind) {
		(NodeKind::RegularFile, NodeKind::RegularFile) => {
			#[cfg(feature = "content-hash")]
			let storage_groups = (shared_storage(l), shared_storage(r));
			let compare_data = true;
			#[cfg(feature = "finder-layout")]
			let compare_data = if finder_layout.is_some_and(|policy| {
				let store = Some(policy.store_name);
				paths.left.last().map(String::as_str) == store
					&& paths.right.last().map(String::as_str) == store
			}) {
				changes.finder_layout = finder_layout::compare_stores(
					(left_view, &left.node),
					(right_view, &right.node),
					paths,
					payloads,
				)?;
				if !changes.finder_layout.is_empty() {
					changes.fields.push(Field::FinderLayout);
				}
				false
			} else {
				compare_data
			};
			if compare_data
				&& !payloads.fork(
					(left_view, &left.node),
					(right_view, &right.node),
					ForkKind::Data,
					paths,
					#[cfg(feature = "content-hash")]
					storage_groups,
				)? {
				changes.fields.push(Field::DataFork);
			}
			if !payloads.fork(
				(left_view, &left.node),
				(right_view, &right.node),
				ForkKind::Resource,
				paths,
				#[cfg(feature = "content-hash")]
				storage_groups,
			)? {
				changes.fields.push(Field::ResourceFork);
			}
		}
		(NodeKind::Symlink, NodeKind::Symlink)
			if !payloads.link((left_view, &left.node), (right_view, &right.node), paths)? =>
		{
			changes.fields.push(Field::SymlinkTarget);
		}
		_ => {}
	}
	Ok(changes)
}

#[cfg(feature = "content-hash")]
fn shared_storage(metadata: &Metadata<StorageId>) -> Option<&StorageId> {
	metadata
		.hard_link
		.as_ref()
		.filter(|link| link.count > NonZeroU32::MIN)
		.map(|link| &link.storage)
}
