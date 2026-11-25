use std::num::NonZeroU32;

use dmgez_vfs::volume::{
	ForkKind, NodeKind, RawName, ViewEntry, ViewError, VolumeBackend, VolumeView,
};

/// Lengths in bytes of the independently admitted forks of a regular file.
/// An engine can report an absent fork or a failure to admit it.
#[derive(Debug)]
#[non_exhaustive]
pub struct ForkSizes<E> {
	/// Logical data length after any supported decompression.
	pub data: Result<Option<u64>, ViewError<E>>,
	/// Length of the stored resource fork.
	pub resource: Result<Option<u64>, ViewError<E>>,
}

/// Storage observations derived from the selected entry's filesystem view.
#[derive(Debug)]
#[non_exhaustive]
pub struct StorageMetadata<E> {
	/// Regular file forks. Other entry kinds have no applicable fork query.
	pub forks: Option<ForkSizes<E>>,
	/// Exact stored attribute names. An empty list indicates no attributes,
	/// and `None` indicates that the backend cannot enumerate them.
	pub xattrs: Result<Option<Vec<RawName>>, ViewError<E>>,
}

/// Queries fork lengths and extended-attribute names without exporting content.
///
/// The node must come from this view. Failures in individual operations stay in
/// the report so an unavailable data fork does not conceal the resource fork or
/// attributes. The backend bounds native reads and may read compression metadata
/// while admitting a logical fork. Attribute names are capped by `attribute_limit`.
pub fn inspect_storage<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	entry: &ViewEntry<B::Id>,
	attribute_limit: NonZeroU32,
) -> StorageMetadata<B::Error> {
	let forks = (entry.kind == NodeKind::RegularFile).then(|| ForkSizes {
		data: view.with_fork(&entry.node, ForkKind::Data, |fork| fork.len()),
		resource: view.with_fork(&entry.node, ForkKind::Resource, |fork| fork.len()),
	});
	let xattrs = view.list_xattrs(&entry.node, attribute_limit);
	StorageMetadata { forks, xattrs }
}
