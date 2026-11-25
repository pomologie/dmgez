#![cfg(feature = "install")]

use std::{io, num::NonZeroU16, vec::IntoIter};

use dmgez_operations::{
	extract::{NameBudget, NamePlanError},
	install::{InstallerError, InstallerLimits, InstallerReport, InstallerRule, check_installer},
};
use dmgez_vfs::{
	VolumeId,
	volume::{
		DirectoryEntry, DirectoryPage, DirectoryReader, ForkKind, ForkReader, LookupName, Metadata,
		NodeKind, RawName, VolumeBackend, VolumeView,
	},
};

const APP: &str = "Witness.APP";
const ROOT_ID: usize = 0;
const APP_ID: usize = 1;
const CONTENTS_ID: usize = 2;
const EXECUTABLE_DIRECTORY_ID: usize = 4;
const EXECUTABLE: &str = "Witness";
const EXECUTABLE_MODE: u16 = 0o755;
const DATA_MODE: u16 = 0o644;
const APPLICATIONS: &str = "Applications";
const APPLICATIONS_TARGET: &[u8] = b"/Applications";
const PLIST: &[u8] =
	br#"<plist><dict><key>CFBundleExecutable</key><string>Witness</string></dict></plist>"#;

struct Object {
	parent: Option<usize>,
	name: &'static str,
	data: Data,
}

enum Data {
	Directory,
	File {
		bytes: &'static [u8],
		mode: u16,
		read: bool,
	},
	Link(Option<&'static [u8]>),
}

impl Object {
	fn child(parent: usize, name: &'static str, data: Data) -> Self {
		Self {
			parent: Some(parent),
			name,
			data,
		}
	}
	fn kind(&self) -> NodeKind {
		match self.data {
			Data::Directory => NodeKind::Directory,
			Data::File { .. } => NodeKind::RegularFile,
			Data::Link(_) => NodeKind::Symlink,
		}
	}
}

struct Fixture(Vec<Object>);

impl Fixture {
	fn app(mode: u16) -> Self {
		Self(vec![
			Object {
				parent: None,
				name: "",
				data: Data::Directory,
			},
			Object::child(ROOT_ID, APP, Data::Directory),
			Object::child(APP_ID, "Contents", Data::Directory),
			Object::child(
				CONTENTS_ID,
				"Info.plist",
				Data::File {
					bytes: PLIST,
					mode: DATA_MODE,
					read: true,
				},
			),
			Object::child(CONTENTS_ID, "MacOS", Data::Directory),
			Object::child(
				EXECUTABLE_DIRECTORY_ID,
				EXECUTABLE,
				Data::File {
					bytes: b"test application executable",
					mode,
					read: false,
				},
			),
		])
	}
	fn push(&mut self, parent: usize, name: &'static str, data: Data) {
		self.0.push(Object::child(parent, name, data));
	}
	fn view(self) -> VolumeView<Self> {
		VolumeView::new(VolumeId::WholeDisk, self)
	}
}

struct Directory(IntoIter<DirectoryEntry<usize>>);

impl DirectoryReader for Directory {
	type Id = usize;
	type Error = io::Error;

	fn next_page(&mut self, limit: NonZeroU16) -> io::Result<DirectoryPage<usize>> {
		let entries = self.0.by_ref().take(usize::from(limit.get())).collect();
		Ok(DirectoryPage {
			entries,
			complete: self.0.as_slice().is_empty(),
		})
	}
}

struct Payload {
	bytes: &'static [u8],
	read: bool,
}

impl ForkReader for Payload {
	type Error = io::Error;

	fn len(&self) -> u64 {
		self.bytes.len() as u64
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<()> {
		assert!(
			self.read,
			"installer check must not read executable contents"
		);
		let offset = usize::try_from(offset).map_err(io::Error::other)?;
		let input = self
			.bytes
			.get(offset..)
			.and_then(|bytes| bytes.get(..output.len()))
			.ok_or(io::ErrorKind::UnexpectedEof)?;
		output.copy_from_slice(input);
		Ok(())
	}
}

impl VolumeBackend for Fixture {
	type Id = usize;
	type Error = io::Error;
	type Directory<'view> = Directory;

	fn root(&mut self) -> io::Result<usize> {
		Ok(ROOT_ID)
	}

	fn lookup(&mut self, _: usize, _: LookupName<'_>) -> io::Result<Option<DirectoryEntry<usize>>> {
		panic!("check must use exact stored names from the directory plan")
	}

	fn stat(&mut self, node: usize) -> io::Result<Option<Metadata<u64>>> {
		let object = &self.0[node];
		let mode = match object.data {
			Data::File { mode, .. } => mode,
			_ => 0,
		};
		Ok(Some(Metadata {
			kind: object.kind(),
			mode,
			uid: 0,
			gid: 0,
			hard_link: None,
		}))
	}

	fn read_dir(&mut self, directory: usize) -> io::Result<Directory> {
		assert!(matches!(self.0[directory].data, Data::Directory));
		let entries: Vec<_> = self
			.0
			.iter()
			.enumerate()
			.filter(|(_, object)| object.parent == Some(directory))
			.map(|(node, object)| DirectoryEntry {
				node,
				name: RawName::Utf16(object.name.encode_utf16().collect()),
				kind: object.kind(),
			})
			.collect();
		Ok(Directory(entries.into_iter()))
	}

	fn with_fork<T>(
		&mut self,
		node: usize,
		kind: ForkKind,
		inspect: impl FnOnce(&mut dyn ForkReader<Error = io::Error>) -> T,
	) -> io::Result<Option<T>> {
		assert_eq!(kind, ForkKind::Data);
		let Data::File { bytes, read, .. } = self.0[node].data else {
			panic!("fork check must not follow links")
		};
		Ok(Some(inspect(&mut Payload { bytes, read })))
	}

	fn read_link(&mut self, node: usize) -> io::Result<Option<Vec<u8>>> {
		let Data::Link(bytes) = self.0[node].data else {
			panic!("only symlinks expose a literal link payload")
		};
		Ok(bytes.map(<[u8]>::to_vec))
	}
}

fn inspect(fixture: Fixture) -> InstallerReport {
	check_installer(&mut fixture.view(), InstallerLimits::default(), || false).unwrap()
}

#[test]
fn verifies_top_level_app_layout_and_exact_shortcut_without_reading_the_executable() {
	let mut fixture = Fixture::app(EXECUTABLE_MODE);
	fixture.push(ROOT_ID, APPLICATIONS, Data::Link(Some(APPLICATIONS_TARGET)));
	fixture.push(
		CONTENTS_ID,
		"Internal",
		Data::Link(Some(b"../Contents/Info.plist")),
	);
	let report = inspect(fixture);
	assert_eq!(report.applications, 1);
	assert!(report.is_valid());
}

#[test]
fn reports_exact_top_level_spelling_invalid_shortcut_and_executable_mode() {
	let mut fixture = Fixture::app(DATA_MODE);
	fixture.push(ROOT_ID, APPLICATIONS, Data::Link(Some(b"/applications")));
	fixture.push(ROOT_ID, "Alias.app", Data::Link(Some(b"Witness.APP")));
	let report = inspect(fixture);
	assert_eq!(report.applications, 1);
	assert!(!report.is_valid());
	assert_eq!(
		report
			.diagnostics
			.iter()
			.map(|finding| (finding.rule, finding.path.as_str()))
			.collect::<Vec<_>>(),
		[
			(InstallerRule::InvalidApplicationEntry, "/Alias.app"),
			(InstallerRule::InvalidApplicationsLink, "/Applications"),
			(InstallerRule::BundleExecutableInvalid, "/Witness.APP"),
		]
	);
	assert_eq!(
		report.diagnostics[1].target.as_deref(),
		Some(b"/applications".as_slice())
	);
	assert_eq!(
		report.diagnostics[2].rule.as_str(),
		"bundle_executable_invalid"
	);
}

#[test]
fn distinguishes_external_links_ambiguous_parent_traversal_and_missing_targets() {
	let mut fixture = Fixture::app(EXECUTABLE_MODE);
	fixture.push(CONTENTS_ID, "Bad", Data::Link(Some(b"../../../outside")));
	fixture.push(
		CONTENTS_ID,
		"Ambiguous",
		Data::Link(Some(b"MacOS/../Info.plist")),
	);
	fixture.push(CONTENTS_ID, "Missing", Data::Link(None));
	let report = inspect(fixture);
	assert_eq!(
		report
			.diagnostics
			.iter()
			.map(|finding| (finding.rule, finding.path.as_str()))
			.collect::<Vec<_>>(),
		[
			(InstallerRule::MissingApplicationsLink, "/Applications"),
			(
				InstallerRule::UnverifiedSymlinkTarget,
				"/Witness.APP/Contents/Ambiguous"
			),
			(InstallerRule::ExternalSymlink, "/Witness.APP/Contents/Bad"),
			(
				InstallerRule::InvalidSymlinkTarget,
				"/Witness.APP/Contents/Missing"
			),
		]
	);
}

#[test]
fn requires_a_real_application_at_the_volume_root() {
	let mut fixture = Fixture::app(EXECUTABLE_MODE);
	fixture.0[APP_ID].name = "Nested";
	fixture.push(APP_ID, APP, Data::Directory);
	let report = inspect(fixture);
	assert_eq!(report.applications, 0);
	assert_eq!(report.diagnostics.len(), 1);
	assert_eq!(
		report.diagnostics[0].rule,
		InstallerRule::MissingApplication
	);
	assert_eq!(report.diagnostics[0].path, "/");
}

#[test]
fn rejects_cancellation_and_bounds_instead_of_reporting_partial_success() {
	let defaults = InstallerLimits::default();
	let mut fixture = Fixture::app(EXECUTABLE_MODE).view();
	assert!(matches!(
		check_installer(&mut fixture, defaults, || true),
		Err(InstallerError::Cancelled)
	));
	let mut names = defaults.names;
	names.max_entries = 0;
	assert!(matches!(
		check_installer(&mut fixture, InstallerLimits { names, ..defaults }, || {
			false
		}),
		Err(InstallerError::Names(NamePlanError::Limit(
			NameBudget::Entries
		)))
	));
	assert!(matches!(
		check_installer(
			&mut fixture,
			InstallerLimits {
				max_applications: 0,
				..defaults
			},
			|| false
		),
		Err(InstallerError::ApplicationLimit)
	));
	assert!(matches!(
		check_installer(
			&mut fixture,
			InstallerLimits {
				max_diagnostics: 0,
				..defaults
			},
			|| false
		),
		Err(InstallerError::DiagnosticLimit)
	));
	let mut fixture = Fixture::app(EXECUTABLE_MODE);
	fixture.push(ROOT_ID, APPLICATIONS, Data::Link(Some(APPLICATIONS_TARGET)));
	assert!(matches!(
		check_installer(&mut fixture.view(), InstallerLimits { max_link_bytes: 0, ..defaults }, || false),
		Err(InstallerError::LinkLimit { path, limit: 0 }) if path == "/Applications"
	));
}
