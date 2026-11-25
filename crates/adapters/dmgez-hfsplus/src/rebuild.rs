//! Plan and seal a fresh HFS+ or HFSX volume from an existing catalog and typed edits.

use std::{
	collections::{BTreeMap, TryReserveError},
	error::Error as StdError,
	io::{self, Read},
	path::Path,
	rc::Rc,
};

use dmgez_builder::manifest::{SourceIdentity, SourceSnapshot};
use dmgez_config::{VirtualPath, project_edit::ProjectEdit};
use dmgez_finder::edit::EditableStore;
use hfsplus_builder::{
	SealedPlan,
	volume::{
		self, CapacityError, CapacityPolicy, EntryKind, EntryMetadata, ForkSource, HardLinkGroup,
		ManifestEntry, VolumeError, VolumeFormat, VolumeOptions,
	},
};
use hfsplus_core::{ByteLen, ByteOffset, RangeError, ReadAt};
use hfsplus_format::volume::CatalogId;
use hfsplus_reader::{Volume, catalog::Catalog, overflow::ResolvedFork};
use thiserror::Error;

mod background;
mod capture;
mod overlay;
mod prepared;

pub use crate::hfsplus::HostCopyMetadataLoss;
pub use capture::CaptureError;
pub use overlay::OverlayError;
pub use prepared::{PrepareError, PreparedEdit};

const IMAGE_SEPARATOR: char = '/';
const VOLUME_DATE_COUNT: usize = 4;
const CREATION_DATE: usize = 0;
const FINDER_INFO_BYTES: usize = 32;

type Attribute = (String, Vec<u8>);
type Attributes = Rc<[Attribute]>;

struct Snapshot<'volume, S> {
	name: String,
	format: VolumeFormat,
	root: EntryMetadata,
	dates: [u32; VOLUME_DATE_COUNT],
	finder_info: [u8; FINDER_INFO_BYTES],
	block_size: u32,
	entries: BTreeMap<VirtualPath, Node<'volume, S>>,
	root_attributes: Vec<Attribute>,
	backgrounds: BTreeMap<VirtualPath, VirtualPath>,
}

struct Node<'volume, S> {
	metadata: EntryMetadata,
	kind: Kind<'volume, S>,
	attributes: Attributes,
}

enum Kind<'volume, S> {
	Directory,
	File {
		file: Rc<File<'volume, S>>,
		// Only effective regular-file hard links carry a shared inode key.
		inode: Option<LinkIdentity>,
	},
	FinderStore {
		file: Rc<File<'volume, S>>,
		layout: EditableStore,
	},
	Symlink(Vec<u8>),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum LinkIdentity {
	Image(CatalogId),
	Host(SourceIdentity),
}

struct File<'volume, S> {
	data: Payload<'volume, S>,
	resource: Option<Payload<'volume, S>>,
}

struct LinkedPaths<'path, 'volume, S> {
	paths: Vec<&'path str>,
	first: &'path Node<'volume, S>,
	file: &'path File<'volume, S>,
}

enum Payload<'volume, S> {
	Image(Rc<ResolvedFork<'volume, S>>),
	Bytes(Rc<[u8]>),
	HostFile(&'volume SourceSnapshot),
	HostBytes(&'volume [u8]),
}

impl<'volume, S> Payload<'volume, S> {
	fn image(fork: ResolvedFork<'volume, S>) -> Self {
		Self::Image(Rc::new(fork))
	}

	fn bytes(bytes: Vec<u8>) -> Self {
		Self::Bytes(bytes.into())
	}
}

impl<S> Clone for Payload<'_, S> {
	fn clone(&self) -> Self {
		match self {
			Self::Image(fork) => Self::Image(Rc::clone(fork)),
			Self::Bytes(bytes) => Self::Bytes(Rc::clone(bytes)),
			Self::HostFile(source) => Self::HostFile(source),
			Self::HostBytes(bytes) => Self::HostBytes(bytes),
		}
	}
}

#[derive(Debug, Error)]
enum PayloadError<E> {
	#[error(transparent)]
	Image(#[from] hfsplus_reader::Error<E>),
	#[error(transparent)]
	Host(#[from] io::Error),
	#[error(transparent)]
	Range(#[from] RangeError),
}

impl<S: ReadAt> ReadAt for Payload<'_, S> {
	type Error = PayloadError<S::Error>;

	fn len(&self) -> ByteLen {
		match self {
			Self::Image(fork) => fork.len(),
			Self::Bytes(bytes) => ReadAt::len(bytes.as_ref()),
			Self::HostFile(source) => ByteLen(source.metadata().size),
			Self::HostBytes(bytes) => ReadAt::len(*bytes),
		}
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> Result<(), Self::Error> {
		match self {
			Self::Image(fork) => fork.read_exact_at(offset, output)?,
			Self::Bytes(bytes) => bytes.as_ref().read_exact_at(offset, output)?,
			Self::HostFile(source) => crate::hfsplus::read_host_source(source, offset, output)?,
			Self::HostBytes(bytes) => bytes.read_exact_at(offset, output)?,
		};
		Ok(())
	}
}

/// Failure planning a loss-checked, copy-on-write HFS+ rebuild.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RebuildError<E: StdError + 'static> {
	#[error("source HFS+ catalog could not be retained")]
	Source(#[from] CaptureError<E>),
	#[error("edit batch could not be applied")]
	Edits(#[from] OverlayError),
	#[error(transparent)]
	Filesystem(#[from] VolumeError),
	#[error(transparent)]
	Capacity(#[from] CapacityError),
	#[error("could not reserve the bounded rebuild manifest")]
	Reserve(#[from] TryReserveError),
}

/// An immutable filesystem image whose original forks are read by position.
pub struct RebuiltVolume<'volume, S> {
	plan: SealedPlan<Payload<'volume, S>>,
	entries: usize,
}

impl<S: ReadAt> RebuiltVolume<'_, S> {
	/// Size of the rebuilt raw volume in bytes.
	#[must_use]
	pub fn logical_bytes(&self) -> u64 {
		self.plan.image_len().0
	}

	/// Number of visible entries below the root after applying the edit batch.
	#[must_use]
	pub const fn entries(&self) -> usize {
		self.entries
	}

	/// Start streaming the sealed image from byte zero.
	#[must_use]
	pub fn reader(&self) -> impl Read + '_
	where
		S::Error: StdError + Send + Sync + 'static,
	{
		self.plan.reader()
	}
}

/// Rebuild the original HFS+ or HFSX namespace from a read-only source volume.
/// The caller must hold the source stable and verify its identity before
/// independent output publication. The source catalog and volume must come
/// from the same admitted native session.
/// # Errors
/// Refuses unsupported source metadata, invalid edits, resource limits, and
/// a rebuilt namespace or capacity the HFS+ writer cannot represent.
pub fn seal<'volume, S: ReadAt>(
	volume: &'volume Volume<S>,
	catalog: &Catalog<'volume, S>,
	edits: &[ProjectEdit],
	changes_dir: &Path,
) -> Result<RebuiltVolume<'volume, S>, RebuildError<S::Error>>
where
	S::Error: StdError + 'static,
{
	let mut snapshot = capture::capture(volume, catalog, capture::MAX_CAPTURE_BYTES)?;
	let backgrounds = overlay::apply(&mut snapshot, edits, changes_dir)?;
	seal_volume(volume, &snapshot, &backgrounds)
}

/// Seal a rebuild with the host inputs captured for this exact edit batch.
/// The caller must revalidate the prepared host sources and the original image
/// after staged verification and before publication.
/// # Errors
/// Refuses unsupported source metadata, invalid or conflicting edits, exhausted
/// budgets, or a final tree the HFS+ writer cannot represent.
pub fn seal_prepared<'volume, S: ReadAt>(
	volume: &'volume Volume<S>,
	catalog: &Catalog<'volume, S>,
	prepared: &'volume PreparedEdit,
) -> Result<RebuiltVolume<'volume, S>, RebuildError<S::Error>>
where
	S::Error: StdError + 'static,
{
	let mut snapshot = capture::capture(volume, catalog, prepared.capture_budget())?;
	let backgrounds =
		overlay::apply_with_copies(&mut snapshot, prepared.edits(), prepared.nodes()?)?;
	seal_volume(volume, &snapshot, &backgrounds)
}

/// Verify that a reopened staged volume satisfies the rebuild profile.
///
/// # Errors
/// Rejects a foreign catalog, unsupported format, or incomplete structural coverage.
pub fn verify_rebuilt_volume<S: ReadAt>(
	volume: &Volume<S>,
	catalog: &Catalog<'_, S>,
) -> Result<(), CaptureError<S::Error>>
where
	S::Error: StdError + 'static,
{
	let format = capture::volume_format(volume, catalog)?;
	capture::check_catalog_structure(volume, format, Default::default())?;
	Ok(())
}

fn seal_volume<'volume, S: ReadAt>(
	volume: &Volume<S>,
	snapshot: &Snapshot<'volume, S>,
	backgrounds: &background::Bindings,
) -> Result<RebuiltVolume<'volume, S>, RebuildError<S::Error>>
where
	S::Error: StdError + 'static,
{
	let entries = snapshot.entries.len();
	let original_bytes = volume.header().geometry.allocation_bytes();
	let plan = seal_snapshot(snapshot, original_bytes, backgrounds)?;
	Ok(RebuiltVolume { plan, entries })
}

fn seal_snapshot<'volume, S: ReadAt>(
	snapshot: &Snapshot<'volume, S>,
	original_bytes: u64,
	backgrounds: &background::Bindings,
) -> Result<SealedPlan<Payload<'volume, S>>, RebuildError<S::Error>>
where
	S::Error: StdError + 'static,
{
	let mut options = VolumeOptions::for_capacity(&snapshot.name, snapshot.block_size)?;
	options.format = snapshot.format;
	options.root = snapshot.root;
	options.dates = snapshot.dates;
	options.finder_info = snapshot.finder_info;
	let mut entries = Vec::new();
	entries.try_reserve_exact(snapshot.entries.len())?;
	let mut attributes = Vec::new();
	attributes.try_reserve_exact(snapshot.entries.len())?;
	let mut hard_links = BTreeMap::<LinkIdentity, LinkedPaths<'_, 'volume, S>>::new();
	for (path, node) in &snapshot.entries {
		let relative = relative_path(path);
		if let Kind::File {
			file,
			inode: Some(inode),
		} = &node.kind
		{
			let group = hard_links.entry(*inode).or_insert_with(|| LinkedPaths {
				paths: Vec::new(),
				first: node,
				file,
			});
			group.paths.try_reserve(1)?;
			group.paths.push(relative);
		} else {
			let mut entry = manifest_entry(relative, node);
			if let Kind::FinderStore { file, .. } = &node.kind
				&& backgrounds.contains_key(path)
			{
				entry.kind = EntryKind::GeneratedFile {
					resource: file.resource.clone().map(ForkSource::whole),
				};
			}
			entries.push(entry);
			attributes.push((relative, node.attributes.as_ref()));
		}
	}
	let mut groups = Vec::new();
	groups.try_reserve_exact(hard_links.len())?;
	for LinkedPaths { paths, first, file } in hard_links.into_values() {
		let path = paths[0];
		attributes.push((path, first.attributes.as_ref()));
		if paths.len() == 1 {
			entries.push(manifest_entry(path, first));
		} else {
			groups.push(HardLinkGroup {
				paths,
				metadata: first.metadata,
				data: ForkSource::whole(file.data.clone()),
				resource: file.resource.clone().map(ForkSource::whole),
			});
		}
	}
	let mut draft = volume::plan_with_hard_links(options, entries, groups)?;
	background::bind(&mut draft, snapshot, backgrounds)?;
	for (name, value) in &snapshot.root_attributes {
		draft.add_inline_attribute("", name, value)?;
	}
	for (path, attributes) in attributes {
		for (name, value) in attributes {
			draft.add_inline_attribute(path, name, value)?;
		}
	}
	let free_space = ByteLen(0);
	let minimum = draft.capacity(CapacityPolicy::Auto { free_space })?;
	let bytes = ByteLen(original_bytes.max(minimum.image_bytes().0));
	Ok(draft.seal_with_capacity(CapacityPolicy::Exact { bytes, free_space })?)
}

fn relative_path(path: &VirtualPath) -> &str {
	// VirtualPath always begins with one ASCII separator.
	&path.as_str()[IMAGE_SEPARATOR.len_utf8()..]
}

fn manifest_entry<'path, 'volume, S: ReadAt>(
	path: &'path str,
	node: &Node<'volume, S>,
) -> ManifestEntry<'path, Payload<'volume, S>> {
	let kind = match &node.kind {
		Kind::Directory => EntryKind::Folder,
		Kind::File { file, .. } | Kind::FinderStore { file, .. } => EntryKind::File {
			data: ForkSource::whole(file.data.clone()),
			resource: file.resource.clone().map(ForkSource::whole),
		},
		Kind::Symlink(target) => EntryKind::Symlink {
			target: target.clone(),
		},
	};
	ManifestEntry {
		path,
		metadata: node.metadata,
		kind,
	}
}
