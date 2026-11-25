use std::{
	fs, io,
	num::{NonZeroU16, NonZeroUsize},
	path::{Path, PathBuf},
};

use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, Fork, ForkKind, ForkReader, LookupName,
		Metadata, NodeKind, RawName, VolumeBackend, VolumeView,
	},
};

use super::super::{
	DestinationDirectory, ForkWriteError, ForkWriteLimits, HostName, StableDestination,
};
use crate::extract::{DEFAULT_COMPONENT_BYTES, NameProfile};

const ROOT_ID: u8 = 1;
const FILE_ID: u8 = 2;
pub(super) const DESTINATION: &str = "file";
pub(super) const NESTED: &str = "nested";
pub(super) const DETACHED: &str = "detached";
pub(super) const DATA: &[u8] = b"data fork";
pub(super) const RESOURCE: &[u8] = b"selected resource fork bytes";
pub(super) const SENTINEL: &[u8] = b"original file remains";
pub(super) const CHUNK_BYTES: NonZeroUsize = NonZeroUsize::new(4).unwrap();
pub(super) const LIMITS: ForkWriteLimits = ForkWriteLimits {
	max_fork_bytes: 4096,
	chunk_bytes: CHUNK_BYTES,
};

pub(super) struct Scratch {
	_directory: tempfile::TempDir,
	pub(super) destination: PathBuf,
	pub(super) outside: PathBuf,
}

impl Scratch {
	pub(super) fn new() -> Self {
		let directory = tempfile::tempdir().unwrap();
		// On macOS the system-provided temporary directory may have a
		// symlink in its prefix. Supply its concrete path to the capability.
		let scope = directory.path().canonicalize().unwrap();
		let destination = scope.join("output");
		let outside = scope.join("outside");
		fs::create_dir(&destination).unwrap();
		fs::create_dir(&outside).unwrap();
		fs::write(outside.join(DESTINATION), SENTINEL).unwrap();
		Self {
			_directory: directory,
			destination,
			outside,
		}
	}

	pub(super) fn open(&self) -> StableDestination {
		StableDestination::open(&self.destination).unwrap()
	}

	pub(super) fn outside_untouched(&self) {
		assert_eq!(fs::read(self.outside.join(DESTINATION)).unwrap(), SENTINEL);
	}
}

pub(super) fn host_name(name: &str) -> HostName {
	HostName::admit(
		RawName::Utf8(name.into()),
		NameProfile::PortableAscii,
		DEFAULT_COMPONENT_BYTES,
	)
	.unwrap()
}

pub(super) fn assert_children(path: &Path, expected: &[&str]) {
	let mut children: Vec<_> = fs::read_dir(path)
		.unwrap()
		.map(|entry| entry.unwrap().file_name().into_string().unwrap())
		.collect();
	children.sort_unstable();
	assert_eq!(children, expected);
}

struct Payload<'data, BeforeRead> {
	bytes: &'data [u8],
	before_read: &'data mut BeforeRead,
}

impl<BeforeRead: FnMut(u64, &mut [u8]) -> io::Result<()>> ForkReader for Payload<'_, BeforeRead> {
	type Error = io::Error;

	fn len(&self) -> u64 {
		self.bytes.len().try_into().unwrap()
	}

	fn read_exact_at(&mut self, offset: u64, destination: &mut [u8]) -> io::Result<()> {
		(self.before_read)(offset, destination)?;
		let index = usize::try_from(offset).unwrap();
		destination.copy_from_slice(&self.bytes[index..index + destination.len()]);
		Ok(())
	}
}

struct SingleFileVolume<'payload, BeforeRead> {
	data: &'payload [u8],
	resource: Option<&'payload [u8]>,
	before_read: BeforeRead,
}

struct SingleFileDirectory;

impl DirectoryReader for SingleFileDirectory {
	type Error = io::Error;
	type Id = u8;

	fn next_page(&mut self, _: NonZeroU16) -> Result<DirectoryPage<u8>, io::Error> {
		panic!("a fork test must use direct lookup")
	}
}

impl<BeforeRead: FnMut(u64, &mut [u8]) -> io::Result<()>> VolumeBackend
	for SingleFileVolume<'_, BeforeRead>
{
	type Id = u8;
	type Error = io::Error;
	type Directory<'view>
		= SingleFileDirectory
	where
		Self: 'view;

	fn root(&mut self) -> Result<u8, io::Error> {
		Ok(ROOT_ID)
	}

	fn lookup(
		&mut self,
		parent: u8,
		name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<u8>>, io::Error> {
		assert_eq!(parent, ROOT_ID);
		assert_eq!(name, LookupName::Utf8(DESTINATION));
		Ok(Some(DirectoryEntry {
			name: RawName::Utf8(DESTINATION.into()),
			node: FILE_ID,
			kind: NodeKind::RegularFile,
		}))
	}

	fn stat(&mut self, _: u8) -> Result<Option<Metadata<u64>>, io::Error> {
		panic!("a selected-fork copy does not query unrelated metadata")
	}

	fn read_dir(&mut self, _: u8) -> Result<SingleFileDirectory, io::Error> {
		panic!("a fork test must use direct lookup")
	}

	fn with_fork<T>(
		&mut self,
		node: u8,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = io::Error>) -> T,
	) -> Result<Option<T>, io::Error> {
		assert_eq!(node, FILE_ID);
		let bytes = match kind {
			ForkKind::Data => Some(self.data),
			ForkKind::Resource => self.resource,
			_ => None,
		};
		Ok(bytes.map(|bytes| {
			read(&mut Payload {
				bytes,
				before_read: &mut self.before_read,
			})
		}))
	}

	fn read_link(&mut self, _: u8) -> Result<Option<Vec<u8>>, io::Error> {
		panic!("the fork read must not follow a link")
	}
}

pub(super) fn with_fork<T>(
	data: &[u8],
	resource: Option<&[u8]>,
	kind: ForkKind,
	before_read: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
	use_fork: impl FnOnce(&mut Fork<'_, io::Error>) -> T,
) -> Option<T> {
	let mut volume = VolumeView::new(
		VolumeId::WholeDisk,
		SingleFileVolume {
			data,
			resource,
			before_read,
		},
	);
	let root = volume.root().unwrap();
	let source = volume
		.lookup(&root, LookupName::Utf8(DESTINATION))
		.unwrap()
		.unwrap();
	volume.with_fork(&source.node, kind, use_fork).unwrap()
}

pub(super) fn copy(
	directory: &DestinationDirectory<'_>,
	limits: ForkWriteLimits,
	before_read: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
	cancelled: impl FnMut() -> bool,
) -> Result<u64, ForkWriteError<io::Error>> {
	with_fork(DATA, Some(RESOURCE), ForkKind::Data, before_read, |fork| {
		directory.publish_fork(&host_name(DESTINATION), fork, limits, cancelled)
	})
	.unwrap()
}
