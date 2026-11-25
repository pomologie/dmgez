#![cfg(feature = "extract")]

use std::{
	cell::Cell,
	io,
	num::{NonZeroU16, NonZeroU32},
};

use dmgez_operations::extract::{
	ExtractionLimits, NamePlanError, PlanError, Selection, SelectionReport, prepare,
};
use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, HardLink, LookupName,
		Metadata, NodeKind, Quarantine, QuarantineScope, RawName, VolumeBackend, VolumeView,
	},
};

const ROOT: u16 = 1;
const FOLDER: u16 = 2;
const FILE: u16 = 3;
const SHARED: u16 = 4;
const EMPTY: u16 = 5;
const LINK: u16 = 6;
const ROOT_FILE: &str = "manual";
const SUBDIRECTORY: &str = "Docs";
const SHARED_FILE: &str = "Shared";
const EMPTY_FILE: &str = "Empty";
const LINK_NAME: &str = "Link";
const FILE_DATA: &[u8] = b"manual";
const SHARED_DATA: &[u8] = b"shared data";
const RESOURCE: &[u8] = b"\x00\xff\xa5";
const ALL_DATA_BYTES: u64 = (FILE_DATA.len() + SHARED_DATA.len()) as u64;
const EXECUTABLE_MODE: u16 = 0o100755;
const SHARED_STORAGE: u64 = 401;
const LINK_COUNT: NonZeroU32 = NonZeroU32::new(2).expect("link count");
const STANDARD_ENTRIES: &[(u16, &str)] = &[(FOLDER, SUBDIRECTORY), (FILE, ROOT_FILE)];
const LINK_ENTRIES: &[(u16, &str)] = &[(FOLDER, SUBDIRECTORY), (LINK, LINK_NAME)];
const COLLIDING_ENTRIES: &[(u16, &str)] = &[(FILE, "Manual"), (SHARED, ROOT_FILE)];
const NESTED_ENTRIES: &[(u16, &str)] = &[(SHARED, SHARED_FILE), (EMPTY, EMPTY_FILE)];

struct Image<'a> {
	root_entries: &'static [(u16, &'static str)],
	reads: &'a Cell<usize>,
	fail_file: bool,
	quarantine: Option<(u16, QuarantineScope, Quarantine)>,
}

fn view<'a>(
	root_entries: &'static [(u16, &'static str)],
	reads: &'a Cell<usize>,
	fail_file: bool,
) -> VolumeView<Image<'a>> {
	VolumeView::new(
		VolumeId::WholeDisk,
		Image {
			root_entries,
			reads,
			fail_file,
			quarantine: None,
		},
	)
}

impl Image<'_> {
	fn entries(&self, id: u16) -> &'static [(u16, &'static str)] {
		match id {
			ROOT => self.root_entries,
			FOLDER => NESTED_ENTRIES,
			_ => &[],
		}
	}
}

fn kind(id: u16) -> NodeKind {
	match id {
		ROOT | FOLDER => NodeKind::Directory,
		LINK => NodeKind::Symlink,
		_ => NodeKind::RegularFile,
	}
}

fn directory_entry(id: u16, name: &str) -> DirectoryEntry<u16> {
	DirectoryEntry {
		name: RawName::Utf8(name.into()),
		node: id,
		kind: kind(id),
	}
}

struct Directory {
	remaining: &'static [(u16, &'static str)],
}

impl DirectoryReader for Directory {
	type Id = u16;
	type Error = io::Error;

	fn next_page(&mut self, limit: NonZeroU16) -> io::Result<DirectoryPage<Self::Id>> {
		let (page, remaining) = self
			.remaining
			.split_at(self.remaining.len().min(usize::from(limit.get())));
		self.remaining = remaining;
		Ok(DirectoryPage {
			entries: page
				.iter()
				.map(|&(id, name)| directory_entry(id, name))
				.collect(),
			complete: remaining.is_empty(),
		})
	}
}

struct Reader<'a> {
	bytes: &'static [u8],
	reads: &'a Cell<usize>,
	fail: bool,
}

impl ForkReader for Reader<'_> {
	type Error = io::Error;

	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<()> {
		self.reads.set(self.reads.get() + 1);
		if self.fail {
			return Err(io::Error::other("fixture payload failure"));
		}
		let start = usize::try_from(offset).map_err(io::Error::other)?;
		let end = start
			.checked_add(output.len())
			.ok_or(io::ErrorKind::UnexpectedEof)?;
		let selected = self
			.bytes
			.get(start..end)
			.ok_or(io::ErrorKind::UnexpectedEof)?;
		output.copy_from_slice(selected);
		Ok(())
	}
}

impl VolumeBackend for Image<'_> {
	type Id = u16;
	type Error = io::Error;
	type Directory<'view>
		= Directory
	where
		Self: 'view;

	fn root(&mut self) -> io::Result<u16> {
		Ok(ROOT)
	}

	fn lookup(
		&mut self,
		parent: u16,
		name: LookupName<'_>,
	) -> io::Result<Option<DirectoryEntry<u16>>> {
		let LookupName::Utf8(name) = name else {
			return Ok(None);
		};
		Ok(self
			.entries(parent)
			.iter()
			.find(|entry| entry.1 == name)
			.map(|&(id, stored)| directory_entry(id, stored)))
	}

	fn stat(&mut self, id: u16) -> io::Result<Option<Metadata<u64>>> {
		Ok(Some(Metadata {
			kind: kind(id),
			mode: EXECUTABLE_MODE,
			uid: 1,
			gid: 1,
			hard_link: (id == SHARED).then_some(HardLink {
				storage: SHARED_STORAGE,
				count: LINK_COUNT,
			}),
		}))
	}

	fn quarantine(&mut self, id: u16, scope: QuarantineScope) -> io::Result<Quarantine> {
		Ok(self
			.quarantine
			.filter(|(selected, requested_scope, _)| *selected == id && *requested_scope == scope)
			.map_or(Quarantine::Clear, |(_, _, status)| status))
	}

	fn read_dir(&mut self, id: u16) -> io::Result<Directory> {
		Ok(Directory {
			remaining: self.entries(id),
		})
	}

	fn with_fork<T>(
		&mut self,
		id: u16,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = io::Error>) -> T,
	) -> io::Result<Option<T>> {
		let bytes = match (id, kind) {
			(FILE, ForkKind::Data) => FILE_DATA,
			(FILE, ForkKind::Resource) => RESOURCE,
			(SHARED, ForkKind::Data) => SHARED_DATA,
			(EMPTY, ForkKind::Data) => &[],
			_ => return Ok(None),
		};
		Ok(Some(read(&mut Reader {
			bytes,
			reads: self.reads,
			fail: id == FILE && self.fail_file,
		})))
	}

	fn read_link(&mut self, _id: u16) -> io::Result<Option<Vec<u8>>> {
		panic!("extraction must not read a source symlink")
	}
}

#[test]
fn plans_tree_without_reading_payload_and_reports_shared_storage_projection() {
	let reads = Cell::new(0);
	let mut view = view(STANDARD_ENTRIES, &reads, false);
	let root = view.root().unwrap();
	let plan = prepare(
		&mut view,
		&root,
		Selection::DataTree,
		ExtractionLimits::default(),
		|| false,
	)
	.unwrap();
	let report = plan.report();
	assert_eq!(report.selection, SelectionReport::DataTree);
	assert_eq!(report.files, 3);
	assert_eq!(report.directories, 2);
	assert_eq!(report.bytes, ALL_DATA_BYTES);
	assert_eq!(report.hard_links_materialized, 1);
	let names_and_sizes = plan
		.forks()
		.map(|fork| (fork.name.unwrap().as_str(), fork.bytes))
		.collect::<Vec<_>>();
	assert_eq!(
		names_and_sizes,
		[
			(ROOT_FILE, FILE_DATA.len() as u64),
			(SHARED_FILE, SHARED_DATA.len() as u64),
			(EMPTY_FILE, 0),
		]
	);
	assert_eq!(reads.get(), 0);
}

#[test]
fn rejects_links_name_collisions_and_total_byte_overflow_during_preflight() {
	let reads = Cell::new(0);
	let mut symlink = view(LINK_ENTRIES, &reads, false);
	let root = symlink.root().unwrap();
	assert!(matches!(
		prepare(
			&mut symlink,
			&root,
			Selection::DataTree,
			ExtractionLimits::default(),
			|| false
		),
		Err(PlanError::EntryKind {
			kind: NodeKind::Symlink,
			..
		})
	));
	let mut colliding = view(COLLIDING_ENTRIES, &reads, false);
	let root = colliding.root().unwrap();
	assert!(matches!(
		prepare(
			&mut colliding,
			&root,
			Selection::DataTree,
			ExtractionLimits::default(),
			|| false
		),
		Err(PlanError::Names(NamePlanError::Collision { .. }))
	));
	let mut bounded = view(STANDARD_ENTRIES, &reads, false);
	let root = bounded.root().unwrap();
	let limits = ExtractionLimits {
		max_total_bytes: ALL_DATA_BYTES - 1,
		..ExtractionLimits::default()
	};
	assert!(matches!(
		prepare(&mut bounded, &root, Selection::DataTree, limits, || false),
		Err(PlanError::TotalBytes)
	));
	assert_eq!(reads.get(), 0);
}

#[test]
fn requires_known_clear_quarantine_for_selection_and_tree_members() {
	let reads = Cell::new(0);
	for (id, scope, status) in [
		(ROOT, QuarantineScope::Ancestors, Quarantine::Unavailable),
		(SHARED, QuarantineScope::Object, Quarantine::Marked),
	] {
		let mut view = VolumeView::new(
			VolumeId::WholeDisk,
			Image {
				root_entries: STANDARD_ENTRIES,
				reads: &reads,
				fail_file: false,
				quarantine: Some((id, scope, status)),
			},
		);
		let root = view.root().unwrap();
		let result = prepare(
			&mut view,
			&root,
			Selection::DataTree,
			ExtractionLimits::default(),
			|| false,
		);
		assert!(matches!(
			(status, result),
			(
				Quarantine::Unavailable,
				Err(PlanError::QuarantineUnavailable { entry: None })
			) | (
				Quarantine::Marked,
				Err(PlanError::Quarantined { entry: Some(_) })
			)
		));
	}
	assert_eq!(reads.get(), 0);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod host {
	use std::{fs, os::unix::fs::PermissionsExt};

	use dmgez_operations::extract::{
		ExecutionFailure, HostName, execute,
		host::{ForkWriteError, StableDestination},
	};

	use super::*;

	const OUTPUT: &str = "Extracted";
	const EXECUTE_BITS: u32 = 0o111;

	fn output_name(limits: ExtractionLimits) -> HostName {
		HostName::new(OUTPUT, limits.profile, limits.names.max_component_bytes).unwrap()
	}

	#[test]
	fn reports_only_confirmed_outputs_when_a_later_fork_read_fails() {
		let scratch = tempfile::tempdir().unwrap();
		let parent = fs::canonicalize(scratch.path()).unwrap();
		let destination = StableDestination::open(&parent).unwrap();
		let reads = Cell::new(0);
		let mut view = view(STANDARD_ENTRIES, &reads, true);
		let root = view.root().unwrap();
		let limits = ExtractionLimits::default();
		let plan = prepare(&mut view, &root, Selection::DataTree, limits, || false).unwrap();
		let name = output_name(limits);
		let error = execute(
			&mut view,
			&plan,
			&destination.directory(),
			&name,
			|| Ok(()),
			|| false,
		)
		.unwrap_err();
		assert!(matches!(
			error.failure(),
			ExecutionFailure::Write(ForkWriteError::Source { offset: 0, .. })
		));
		let progress = error.progress();
		assert_eq!(progress.directories_created, 2);
		assert_eq!(progress.files_published, 2);
		assert_eq!(progress.bytes_published, SHARED_DATA.len() as u64);
		assert!(!progress.directory_creation_unconfirmed);
		let output = parent.join(OUTPUT);
		let shared = output.join(SUBDIRECTORY).join(SHARED_FILE);
		assert_eq!(fs::read(&shared).unwrap(), SHARED_DATA);
		assert_eq!(
			fs::metadata(shared).unwrap().permissions().mode() & EXECUTE_BITS,
			0
		);
		assert!(
			fs::read(output.join(SUBDIRECTORY).join(EMPTY_FILE))
				.unwrap()
				.is_empty()
		);
		assert_eq!(fs::read_dir(output).unwrap().count(), 1);
	}

	#[test]
	fn revalidates_after_streaming_and_keeps_resource_fork_output_nonexecutable() {
		let scratch = tempfile::tempdir().unwrap();
		let parent = fs::canonicalize(scratch.path()).unwrap();
		let destination = StableDestination::open(&parent).unwrap();
		let reads = Cell::new(0);
		let mut view = view(STANDARD_ENTRIES, &reads, false);
		let root = view.root().unwrap();
		let entry = view
			.lookup(&root, LookupName::Utf8(ROOT_FILE))
			.unwrap()
			.unwrap();
		let limits = ExtractionLimits::default();
		let plan = prepare(
			&mut view,
			&entry.node,
			Selection::Fork(ForkKind::Resource),
			limits,
			|| false,
		)
		.unwrap();
		let name = output_name(limits);
		let verify = || {
			if reads.get() == 0 {
				Ok(())
			} else {
				Err(io::Error::other("source changed after data was staged"))
			}
		};
		let error = execute(
			&mut view,
			&plan,
			&destination.directory(),
			&name,
			verify,
			|| false,
		)
		.unwrap_err();
		assert!(matches!(
			error.failure(),
			ExecutionFailure::Write(ForkWriteError::SourceIdentity(_))
		));
		assert_eq!(error.progress().files_published, 0);
		assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);

		let progress = execute(
			&mut view,
			&plan,
			&destination.directory(),
			&name,
			|| Ok(()),
			|| false,
		)
		.unwrap();
		assert_eq!(progress.bytes_published, RESOURCE.len() as u64);
		let output = parent.join(OUTPUT);
		assert_eq!(fs::read(&output).unwrap(), RESOURCE);
		assert_eq!(
			fs::metadata(output).unwrap().permissions().mode() & EXECUTE_BITS,
			0
		);
	}
}
