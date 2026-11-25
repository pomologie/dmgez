//! Resolves typed project content into a bounded host manifest.

use std::{
	fs::File,
	io,
	path::{Path, PathBuf},
};

use dmgez_config::{Content, Coordinate, LocalPath, Payload, Project, VirtualPath};
use thiserror::Error;

mod catalog;
#[cfg(unix)]
mod host;
#[cfg(windows)]
#[path = "manifest/host_windows.rs"]
mod host;
#[cfg(any(unix, windows))]
mod walk;

use catalog::{Body, Catalog, Origin};

const PATH_SEPARATOR: char = '/';
const MAX_SYMLINK_BYTES: usize = 4096;
const DEFAULT_MAX_ENTRIES: u64 = 1_000_000;
const DEFAULT_MAX_METADATA_BYTES: u64 = 128 << 20;
const DEFAULT_MAX_DEPTH: usize = 64;

#[cfg(unix)]
use std::sync::Arc;

/// Independent bounds on discovery and owned manifest data.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	/// Maximum retained entries, including synthesized directories.
	pub max_entries: u64,
	/// Maximum host entries examined, including entries rejected by filters.
	pub max_visited: u64,
	/// Maximum virtual path depth and recursive source descent.
	pub max_depth: usize,
	/// Maximum accounted manifest metadata, excluding file contents.
	pub max_metadata_bytes: u64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_entries: DEFAULT_MAX_ENTRIES,
			max_visited: DEFAULT_MAX_ENTRIES,
			max_depth: DEFAULT_MAX_DEPTH,
			max_metadata_bytes: DEFAULT_MAX_METADATA_BYTES,
		}
	}
}

/// Resolved entries in deterministic virtual path order.
#[derive(Debug)]
pub struct Manifest {
	entries: Vec<Entry>,
	auxiliary_sources: Vec<SourceSnapshot>,
	regular_payload_bytes: u64,
	metadata_bytes: u64,
}

impl Manifest {
	/// Returns entries in path order, with a parent before its children.
	///
	/// The target filesystem applies its own catalog order and name equivalence.
	#[must_use]
	pub fn entries(&self) -> &[Entry] {
		&self.entries
	}

	/// Sum of regular file sizes, before allocating forks or grouping links.
	#[must_use]
	pub fn regular_payload_bytes(&self) -> u64 {
		self.regular_payload_bytes
	}

	/// Accounted size of the retained manifest metadata in bytes.
	#[must_use]
	pub fn metadata_bytes(&self) -> u64 {
		self.metadata_bytes
	}

	pub(crate) fn sources(&self) -> impl Iterator<Item = &SourceSnapshot> {
		self.entries
			.iter()
			.filter_map(|entry| entry.source.as_ref())
			.chain(&self.auxiliary_sources)
	}

	#[cfg(feature = "volume-icon")]
	pub(crate) fn auxiliary_source(&self, index: usize) -> &SourceSnapshot {
		&self.auxiliary_sources[index]
	}

	#[cfg(feature = "volume-icon")]
	pub(crate) fn capture_auxiliary_regular(
		&mut self,
		input: &LocalPath,
		config_dir: &Path,
		output: Option<&LocalPath>,
		max_metadata_bytes: u64,
	) -> Result<usize, Error> {
		#[cfg(not(any(unix, windows)))]
		{
			let _ = (input, config_dir, output, max_metadata_bytes);
			Err(Error::UnsupportedHost)
		}
		#[cfg(any(unix, windows))]
		{
			let path = input.resolve(config_dir);
			let anchor = host::Anchor::new()?;
			let leaf = anchor.locate(&path)?;
			let snapshot = anchor.snapshot(&leaf.directory, leaf.name, path.clone())?;
			if snapshot.kind != SourceKind::RegularFile {
				return Err(Error::RegularSourceRequired { path });
			}
			if let Some(output) = output {
				anchor
					.probe_output(&output.resolve(config_dir))?
					.check(&snapshot)?;
			}
			let delta = std::mem::size_of::<SourceSnapshot>()
				.checked_add(path.as_os_str().len())
				.and_then(|bytes| u64::try_from(bytes).ok())
				.ok_or(Error::Limit(Budget::Metadata))?;
			let next = self
				.metadata_bytes
				.checked_add(delta)
				.filter(|&bytes| bytes <= max_metadata_bytes)
				.ok_or(Error::Limit(Budget::Metadata))?;
			self.auxiliary_sources.try_reserve_exact(1)?;
			let index = self.auxiliary_sources.len();
			self.auxiliary_sources.push(snapshot);
			self.metadata_bytes = next;
			Ok(index)
		}
	}
}

/// One virtual entry and its source snapshot when copied from the host.
#[derive(Debug)]
pub struct Entry {
	pub destination: VirtualPath,
	pub kind: EntryKind,
	pub source: Option<SourceSnapshot>,
	pub finder: FinderOverrides,
}

/// Supported manifest entry kinds.
#[derive(Debug)]
#[non_exhaustive]
pub enum EntryKind {
	Directory,
	File,
	/// The exact link bytes, which are never passed to host path resolution.
	Symlink(Vec<u8>),
}

/// Explicit Finder settings from a content declaration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FinderOverrides {
	pub position: Option<[Coordinate; 2]>,
	pub hidden: Option<bool>,
	pub hide_extension: Option<bool>,
	pub custom_icon: Option<LocalPath>,
}

impl From<&Content> for FinderOverrides {
	fn from(entry: &Content) -> Self {
		Self {
			position: entry.position,
			hidden: entry.hidden,
			hide_extension: entry.hide_extension,
			custom_icon: entry.custom_icon.clone(),
		}
	}
}

/// One host filesystem identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceIdentity {
	pub device: u64,
	pub inode: u64,
}

/// A signed Unix timestamp with subsecond precision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timestamp {
	pub seconds: i64,
	pub nanoseconds: u32,
}

/// Stat metadata at the point of discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceMetadata {
	pub identity: SourceIdentity,
	pub size: u64,
	/// POSIX mode, when observed on the source host.
	pub permissions: Option<u16>,
	/// POSIX user and group IDs, when observed on the source host.
	pub ownership: Option<(u32, u32)>,
	pub link_count: u64,
	pub accessed: Option<Timestamp>,
	pub modified: Option<Timestamp>,
	pub changed: Option<Timestamp>,
	pub created: Option<Timestamp>,
	pub flags: Option<u32>,
}

/// An identity-checked host source descriptor.
#[derive(Clone, Debug)]
pub struct SourceSnapshot {
	path: PathBuf,
	metadata: SourceMetadata,
	#[cfg(unix)]
	root: Arc<rustix::fd::OwnedFd>,
	#[cfg(any(unix, windows))]
	kind: SourceKind,
	#[cfg(windows)]
	attributes: u32,
}

#[cfg(any(unix, windows))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceKind {
	Directory,
	RegularFile,
	#[cfg(unix)]
	Symlink,
	#[cfg(unix)]
	Other,
}

#[cfg(any(unix, windows))]
#[derive(Default)]
struct OutputProbe {
	parent: Option<SourceIdentity>,
	output: Option<SourceIdentity>,
}

#[cfg(any(unix, windows))]
impl OutputProbe {
	fn check(&self, source: &SourceSnapshot) -> Result<(), Error> {
		let identity = Some(source.metadata.identity);
		if self.output == identity
			|| (source.kind == SourceKind::Directory && self.parent == identity)
		{
			return Err(Error::OutputSourceAlias {
				path: source.path.clone(),
			});
		}
		Ok(())
	}
}

impl SourceSnapshot {
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	pub(crate) fn is_directory(&self) -> bool {
		self.kind == SourceKind::Directory
	}

	/// Returns the diagnostic path supplied by discovery.
	#[must_use]
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Returns the captured metadata.
	#[must_use]
	pub fn metadata(&self) -> &SourceMetadata {
		&self.metadata
	}

	/// Opens the original regular source through no-follow directory handles.
	///
	/// # Errors
	/// Fails if the source was replaced, changed or cannot be opened safely.
	pub fn open_regular(&self) -> Result<File, Error> {
		#[cfg(any(unix, windows))]
		{
			host::open_regular(self)
		}
		#[cfg(not(any(unix, windows)))]
		{
			Err(Error::UnsupportedHost)
		}
	}

	/// Checks the source location and metadata captured during discovery.
	///
	/// # Errors
	/// Fails if the source changed, vanished or cannot be checked.
	pub fn validate(&self) -> Result<(), Error> {
		#[cfg(any(unix, windows))]
		{
			host::validate(self)
		}
		#[cfg(not(any(unix, windows)))]
		{
			Err(Error::UnsupportedHost)
		}
	}
}

/// Bounded discovery or source access failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error("manifest discovery requires an absolute configuration directory")]
	RelativeConfigDirectory,
	#[error("host source discovery is unavailable on this platform")]
	UnsupportedHost,
	#[error("unsupported host source kind at {path:?}")]
	UnsupportedSource { path: PathBuf },
	#[error("unsupported content declaration")]
	UnsupportedContent,
	#[error("source path cannot be represented in the virtual tree: {path:?}")]
	InvalidSourceName { path: PathBuf },
	#[error("a content declaration cannot replace the image root")]
	RootDestination,
	#[error("regular source required at {path:?}")]
	RegularSourceRequired { path: PathBuf },
	#[error("source changed since discovery: {path:?}")]
	SourceChanged { path: PathBuf },
	#[error("output aliases a captured source or is inside a copied directory: {path:?}")]
	OutputSourceAlias { path: PathBuf },
	#[error("output path is not a regular file: {path:?}")]
	UnsupportedOutput { path: PathBuf },
	#[error("source patterns require a recursively copied directory: {path:?}")]
	PatternsRequireDirectory { path: PathBuf },
	#[cfg(any(unix, windows))]
	#[error("invalid source glob {index} in {list:?}")]
	InvalidPattern {
		list: PatternList,
		index: usize,
		#[source]
		source: globset::Error,
	},
	#[cfg(any(unix, windows))]
	#[error("source glob set for {list:?} exceeds its compilation budget")]
	PatternSet {
		list: PatternList,
		#[source]
		source: globset::Error,
	},
	#[error("multiple incompatible entries for destination {path:?}")]
	DestinationConflict { path: VirtualPath },
	#[error("destination has a non-directory parent: {path:?}")]
	ParentConflict { path: VirtualPath },
	#[error("manifest discovery exceeded the {0:?} budget")]
	Limit(Budget),
	#[error("manifest discovery was cancelled")]
	Cancelled,
	#[error("cannot reserve manifest memory")]
	Reserve(#[from] std::collections::TryReserveError),
	#[error("cannot {operation} {path:?}")]
	Io {
		operation: &'static str,
		path: PathBuf,
		#[source]
		source: io::Error,
	},
}

/// The list containing an invalid source filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatternList {
	Include,
	Exclude,
}

/// Independently checked discovery resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Budget {
	Entries,
	Visited,
	Depth,
	Metadata,
	Payload,
	Symlink,
	Patterns,
}

/// Resolves project contents without reading regular file payloads.
///
/// # Errors
/// Fails on invalid, conflicting, unreachable or unsupported paths and when
/// a discovery budget is exceeded.
pub fn discover(project: &Project, config_dir: &Path, limits: Limits) -> Result<Manifest, Error> {
	discover_with_cancellation(project, config_dir, limits, || false)
}

/// Resolves project contents with cancellation between source entries.
///
/// # Errors
/// Returns discovery errors and stops early when `cancelled` requests it.
pub fn discover_with_cancellation(
	project: &Project,
	config_dir: &Path,
	limits: Limits,
	cancelled: impl FnMut() -> bool,
) -> Result<Manifest, Error> {
	discover_for_output(
		project,
		config_dir,
		limits,
		Some(&project.image.output),
		cancelled,
	)
}

pub(crate) fn discover_for_output(
	project: &Project,
	config_dir: &Path,
	limits: Limits,
	output: Option<&LocalPath>,
	mut cancelled: impl FnMut() -> bool,
) -> Result<Manifest, Error> {
	if !config_dir.is_absolute() {
		return Err(Error::RelativeConfigDirectory);
	}
	let mut catalog = Catalog::new(limits);
	#[cfg(any(unix, windows))]
	let mut walker = walk::Walker::new(config_dir, output);
	#[cfg(not(any(unix, windows)))]
	let _ = output;
	for content in &project.contents {
		if cancelled() {
			return Err(Error::Cancelled);
		}
		catalog.validate_destination(&content.destination)?;
		let finder = FinderOverrides::from(content);
		match &content.payload {
			Payload::Directory => catalog.admit(
				content.destination.clone(),
				Body::generated(EntryKind::Directory, finder, Origin::DeclaredDirectory),
			)?,
			Payload::Symlink { target } => {
				let bytes = target.as_str().as_bytes();
				if bytes.len() > MAX_SYMLINK_BYTES {
					return Err(Error::Limit(Budget::Symlink));
				}
				catalog.admit(
					content.destination.clone(),
					Body::generated(EntryKind::Symlink(bytes.to_vec()), finder, Origin::Other),
				)?;
			}
			Payload::Copy(copy) => {
				#[cfg(any(unix, windows))]
				walker.copy(
					&content.destination,
					copy,
					finder,
					&mut catalog,
					&mut cancelled,
				)?;
				#[cfg(not(any(unix, windows)))]
				{
					let _ = copy;
					return Err(Error::UnsupportedHost);
				}
			}
			_ => return Err(Error::UnsupportedContent),
		}
	}
	catalog.finish()
}
