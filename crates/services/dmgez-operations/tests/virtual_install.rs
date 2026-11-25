#![cfg(feature = "install")]

use std::{cell::Cell, io, num::NonZeroU16};

use dmgez_operations::{
	extract::{NameLimits, NameProfile},
	install::{Error, RequiredMember, prepare_application},
};
use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, LookupName, Metadata,
		NodeKind, RawName, ViewError, VolumeBackend, VolumeView,
	},
};

const APP_NAME: &str = "Installer.app";
const SELECTED_APP: &str = "/installer.app";
const LINK_APP: &str = "/Alias.app";
const CONTENTS_NAME: &str = "Contents";
const INFO_NAME: &str = "Info.plist";
const MACOS_NAME: &str = "MacOS";
const EXECUTABLE_NAME: &str = "Installer";
const EXECUTABLE_MODE: u16 = 0o755;
const DATA_MODE: u16 = 0o644;
const PROGRAM_BYTES: &[u8] = b"#!/bin/false\n";
const INFO: &[u8] = br#"<plist version="1.0"><dict>
	<key>CFBundleExecutable</key><string>Installer</string>
	<key>Nested</key><array><dict><key>CFBundleExecutable</key><string>Other</string></dict></array>
</dict></plist>"#;
const DUPLICATE_KEY: &[u8] = br#"<plist version="1.0"><dict>
	<key>CFBundleExecutable</key><string>Installer</string>
	<key>CFBundleExecutable</key><string>Other</string>
</dict></plist>"#;
const ESCAPING_EXECUTABLE: &[u8] = br#"<plist version="1.0"><dict>
	<key>CFBundleExecutable</key><string>../Installer</string>
</dict></plist>"#;
const WRONG_XML_KEY: &[u8] = br#"<plist><dict>
	<string>CFBundleExecutable</string><string>Installer</string>
</dict></plist>"#;
const WRONG_XML_VALUE: &[u8] = br#"<plist><dict>
	<key>CFBundleExecutable</key><key>Installer</key>
</dict></plist>"#;
const MISMATCHED_CDATA: &[u8] = br#"<plist><dict>
	<key>CFBundleExecutable</key><string>Install<![CDATA[Wrong]]>er</string>
</dict></plist>"#;
const UNKNOWN_ENTITY: &[u8] = br#"<plist><dict>
	<key>CFBundleExecutable</key><string>Install&ignored;er</string>
</dict></plist>"#;
const VALID_CHARACTER_REFERENCE: &[u8] = br#"<plist><dict>
	<key>CFBundleExecutable</key><string>Install&#101;r</string>
</dict></plist>"#;
const VALID_CDATA: &[u8] = br#"<plist><dict>
	<key>CFBundleExecutable</key><string>Instal<![CDATA[l]]>er</string>
</dict></plist>"#;
const OVER_BUDGET_INFO_BYTES: usize = 1 << 20;
const QUALIFIED_PROFILE: NameProfile = NameProfile::PortableAscii;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Node {
	Root,
	Application,
	SourceLink,
	Contents,
	Info,
	MacOs,
	Executable,
	BundleLink,
}

impl Node {
	fn kind(self) -> NodeKind {
		match self {
			Self::Root | Self::Application | Self::Contents | Self::MacOs => NodeKind::Directory,
			Self::Info | Self::Executable => NodeKind::RegularFile,
			Self::SourceLink | Self::BundleLink => NodeKind::Symlink,
		}
	}
}

const ROOT_ENTRIES: &[(Node, &str)] = &[
	(Node::Application, APP_NAME),
	(Node::SourceLink, "Alias.app"),
];
const APP_ENTRIES: &[(Node, &str)] = &[(Node::Contents, CONTENTS_NAME)];
const CONTENTS_ENTRIES: &[(Node, &str)] = &[
	(Node::Info, INFO_NAME),
	(Node::MacOs, MACOS_NAME),
	(Node::BundleLink, "Library"),
];
const PROGRAM_ENTRIES: &[(Node, &str)] = &[(Node::Executable, EXECUTABLE_NAME)];

struct Fixture<'test> {
	info: Option<&'test [u8]>,
	program: Option<&'test [u8]>,
	mode: u16,
	read_count: &'test Cell<usize>,
}

impl<'test> Fixture<'test> {
	fn new(read_count: &'test Cell<usize>) -> Self {
		Self {
			info: Some(INFO),
			program: Some(PROGRAM_BYTES),
			mode: EXECUTABLE_MODE,
			read_count,
		}
	}

	fn view(self) -> VolumeView<Self> {
		VolumeView::new(VolumeId::WholeDisk, self)
	}
}

fn entries(node: Node) -> &'static [(Node, &'static str)] {
	match node {
		Node::Root => ROOT_ENTRIES,
		Node::Application => APP_ENTRIES,
		Node::Contents => CONTENTS_ENTRIES,
		Node::MacOs => PROGRAM_ENTRIES,
		_ => &[],
	}
}

fn entry(&(node, name): &(Node, &str)) -> DirectoryEntry<Node> {
	DirectoryEntry {
		name: RawName::Utf16(name.encode_utf16().collect()),
		node,
		kind: node.kind(),
	}
}

struct Directory(&'static [(Node, &'static str)]);

impl DirectoryReader for Directory {
	type Id = Node;
	type Error = io::Error;

	fn next_page(&mut self, limit: NonZeroU16) -> io::Result<DirectoryPage<Node>> {
		let (page, remaining) = self.0.split_at(self.0.len().min(usize::from(limit.get())));
		self.0 = remaining;
		Ok(DirectoryPage {
			entries: page.iter().map(entry).collect(),
			complete: remaining.is_empty(),
		})
	}
}

struct Data<'test> {
	bytes: &'test [u8],
	read_count: &'test Cell<usize>,
}

impl ForkReader for Data<'_> {
	type Error = io::Error;

	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<()> {
		self.read_count.set(self.read_count.get() + 1);
		let offset = usize::try_from(offset).map_err(io::Error::other)?;
		let source = self
			.bytes
			.get(offset..)
			.and_then(|tail| tail.get(..output.len()))
			.ok_or(io::ErrorKind::UnexpectedEof)?;
		output.copy_from_slice(source);
		Ok(())
	}
}

impl VolumeBackend for Fixture<'_> {
	type Id = Node;
	type Error = io::Error;
	type Directory<'view>
		= Directory
	where
		Self: 'view;

	fn root(&mut self) -> io::Result<Node> {
		Ok(Node::Root)
	}

	fn lookup(
		&mut self,
		parent: Node,
		name: LookupName<'_>,
	) -> io::Result<Option<DirectoryEntry<Node>>> {
		let LookupName::Utf8(name) = name else {
			return Ok(None);
		};
		Ok(entries(parent)
			.iter()
			.find(|(_, stored)| stored.eq_ignore_ascii_case(name))
			.map(entry))
	}

	fn stat(&mut self, node: Node) -> io::Result<Option<Metadata<u64>>> {
		Ok(Some(Metadata {
			kind: node.kind(),
			mode: if node == Node::Executable {
				self.mode
			} else {
				DATA_MODE
			},
			uid: 0,
			gid: 0,
			hard_link: None,
		}))
	}

	fn read_dir(&mut self, directory: Node) -> io::Result<Directory> {
		Ok(Directory(entries(directory)))
	}

	fn with_fork<T>(
		&mut self,
		node: Node,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = io::Error>) -> T,
	) -> io::Result<Option<T>> {
		let bytes = match (node, kind) {
			(Node::Info, ForkKind::Data) => self.info,
			(Node::Executable, ForkKind::Data) => self.program,
			_ => None,
		};
		Ok(bytes.map(|bytes| {
			read(&mut Data {
				bytes,
				read_count: self.read_count,
			})
		}))
	}

	fn read_link(&mut self, _: Node) -> io::Result<Option<Vec<u8>>> {
		Err(io::Error::other(
			"preflight must retain links without reading their targets",
		))
	}
}

#[test]
fn plans_only_the_selected_application_and_retains_its_stored_spelling_and_links() {
	let reads = Cell::new(0);
	let mut view = Fixture::new(&reads).view();
	let source = SELECTED_APP.parse().unwrap();
	let plan = prepare_application(
		&mut view,
		&source,
		QUALIFIED_PROFILE,
		NameLimits::default(),
		|| false,
	)
	.unwrap();
	assert_eq!(plan.destination_name().as_str(), APP_NAME);
	assert_eq!(plan.source().native_id(), Node::Application);
	assert_eq!(plan.executable().native_id(), Node::Executable);
	assert_eq!(plan.names().entries().len(), 5);
	assert!(plan.names().entries().iter().any(|entry| {
		entry.kind == NodeKind::Symlink && entry.source.native_id() == Node::BundleLink
	}));
	assert_eq!(reads.get(), 1);
	let mut second_view = Fixture::new(&reads).view();
	assert!(matches!(
		second_view.stat(plan.source()),
		Err(ViewError::ForeignHandle)
	));
}

#[test]
fn rejects_link_selection_and_executable_name_ambiguity_or_escape() {
	let reads = Cell::new(0);
	let mut view = Fixture::new(&reads).view();
	let link = LINK_APP.parse().unwrap();
	assert!(matches!(
		prepare_application(
			&mut view,
			&link,
			QUALIFIED_PROFILE,
			NameLimits::default(),
			|| false
		),
		Err(Error::ExpectedApplication)
	));
	assert_eq!(reads.get(), 0);
	let source = SELECTED_APP.parse().unwrap();
	for info in [DUPLICATE_KEY, ESCAPING_EXECUTABLE] {
		let mut view = Fixture {
			info: Some(info),
			..Fixture::new(&reads)
		}
		.view();
		assert!(matches!(
			prepare_application(
				&mut view,
				&source,
				QUALIFIED_PROFILE,
				NameLimits::default(),
				|| false
			),
			Err(Error::Info)
		));
	}
}

#[test]
fn admits_a_bounded_binary_info_plist() {
	let mut info = plist::Dictionary::new();
	info.insert("CFBundleExecutable".into(), EXECUTABLE_NAME.into());
	let mut bytes = Vec::new();
	plist::Value::Dictionary(info)
		.to_writer_binary(&mut bytes)
		.unwrap();
	let reads = Cell::new(0);
	let mut view = Fixture {
		info: Some(&bytes),
		..Fixture::new(&reads)
	}
	.view();
	let plan = prepare_application(
		&mut view,
		&SELECTED_APP.parse().unwrap(),
		QUALIFIED_PROFILE,
		NameLimits::default(),
		|| false,
	)
	.unwrap();
	assert_eq!(plan.executable().native_id(), Node::Executable);
	assert_eq!(reads.get(), 1);
}

#[test]
fn validates_xml_roles_and_retains_character_reference_and_cdata_text() {
	let reads = Cell::new(0);
	let source = SELECTED_APP.parse().unwrap();
	let prepare = |info| {
		let mut view = Fixture {
			info: Some(info),
			..Fixture::new(&reads)
		}
		.view();
		prepare_application(
			&mut view,
			&source,
			QUALIFIED_PROFILE,
			NameLimits::default(),
			|| false,
		)
	};
	for info in [WRONG_XML_KEY, WRONG_XML_VALUE, UNKNOWN_ENTITY] {
		assert!(matches!(prepare(info), Err(Error::Info)));
	}
	for info in [VALID_CHARACTER_REFERENCE, VALID_CDATA] {
		assert!(prepare(info).is_ok());
	}
	assert!(matches!(
		prepare(MISMATCHED_CDATA),
		Err(Error::BundleMember(RequiredMember::MainExecutable))
	));
}

#[test]
fn checks_executable_kind_mode_and_fork_presence() {
	let reads = Cell::new(0);
	let source = SELECTED_APP.parse().unwrap();
	let mut missing = Fixture {
		program: None,
		..Fixture::new(&reads)
	}
	.view();
	let mut not_executable = Fixture {
		mode: DATA_MODE,
		..Fixture::new(&reads)
	}
	.view();
	for view in [&mut missing, &mut not_executable] {
		assert!(matches!(
			prepare_application(
				view,
				&source,
				QUALIFIED_PROFILE,
				NameLimits::default(),
				|| false
			),
			Err(Error::Executable)
		));
	}
	let mut wrong_name = Fixture {
		info: Some(br#"<plist><dict><key>CFBundleExecutable</key><string>installer</string></dict></plist>"#),
		..Fixture::new(&reads)
	}
	.view();
	assert!(matches!(
		prepare_application(
			&mut wrong_name,
			&source,
			QUALIFIED_PROFILE,
			NameLimits::default(),
			|| false
		),
		Err(Error::BundleMember(RequiredMember::MainExecutable))
	));
}

#[test]
fn applies_bounds_and_cancellation_before_reading_bundle_bytes() {
	let bytes = vec![0; OVER_BUDGET_INFO_BYTES];
	let reads = Cell::new(0);
	let mut view = Fixture {
		info: Some(&bytes),
		..Fixture::new(&reads)
	}
	.view();
	let source = SELECTED_APP.parse().unwrap();
	assert!(matches!(
		prepare_application(
			&mut view,
			&source,
			QUALIFIED_PROFILE,
			NameLimits::default(),
			|| false
		),
		Err(Error::InfoLimit)
	));
	assert!(matches!(
		prepare_application(
			&mut view,
			&source,
			QUALIFIED_PROFILE,
			NameLimits::default(),
			|| true
		),
		Err(Error::Cancelled)
	));
	assert_eq!(reads.get(), 0);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn refuses_dangling_output_links_and_replaced_destination_directories() {
	use dmgez_operations::extract::host::{HostError, StableDestination};
	use std::{fs, os::unix::fs::symlink};

	let reads = Cell::new(0);
	let mut view = Fixture::new(&reads).view();
	let plan = prepare_application(
		&mut view,
		&SELECTED_APP.parse().unwrap(),
		QUALIFIED_PROFILE,
		NameLimits::default(),
		|| false,
	)
	.unwrap();
	let parent = tempfile::tempdir().unwrap();
	let path = parent.path().canonicalize().unwrap().join("destination");
	fs::create_dir(&path).unwrap();
	let destination = StableDestination::open(&path).unwrap();
	plan.require_fresh_destination(&destination).unwrap();
	assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
	let output = path.join(APP_NAME);
	symlink("missing", &output).unwrap();
	assert!(matches!(
		plan.require_fresh_destination(&destination),
		Err(HostError::Conflict)
	));
	assert!(output.symlink_metadata().unwrap().is_symlink());
	fs::rename(&path, path.with_extension("previous")).unwrap();
	fs::create_dir(&path).unwrap();
	assert!(matches!(
		plan.require_fresh_destination(&destination),
		Err(HostError::Detached)
	));
}
