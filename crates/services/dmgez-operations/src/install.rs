//! Plans a fresh mountless application installation from one selected volume.

mod bundle;
mod check;
mod copy_plan;

pub use check::{
	InstallerDiagnostic, InstallerError, InstallerLimits, InstallerReport, InstallerRule,
	check_installer,
};
pub use copy_plan::{
	CopyAttribute, CopyContent, CopyEntry, CopyLimits, CopyPlan, CopyPlanError, CopySummary,
	prepare_copy,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod execute;
#[cfg(target_os = "macos")]
pub use execute::{ExistingBundleError, verify_existing_bundle};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use execute::{
	HostMetadataError, InstallError, InstallFailure, execute_copy, execute_copy_with_check,
};

use std::{collections::TryReserveError, error::Error as StdError, hash::Hash};

use dmgez_config::VirtualPath;
use dmgez_vfs::volume::{
	ForkKind, LookupName, NodeHandle, NodeKind, ViewEntry, ViewError, VolumeBackend, VolumeView,
};
use thiserror::Error;

use crate::extract::{
	EntryId, HostName, NameError, NameLimits, NamePlan, NamePlanError, NameProfile, preflight_names,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::extract::host::{HostError, StableDestination};

const APPLICATION_EXTENSION: &str = "app";
const EXTENSION_SEPARATOR: char = '.';
const CONTENTS_NAME: &str = "Contents";
const INFO_PLIST_NAME: &str = "Info.plist";
const EXECUTABLE_DIRECTORY_NAME: &str = "MacOS";
const EXECUTE_MODE_BITS: u16 = 0o111;
const MAX_INFO_BYTES: usize = 256 << 10;

/// Bundle path component that must exist with its expected kind and spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RequiredMember {
	Contents,
	InfoPlist,
	ExecutableDirectory,
	MainExecutable,
}

/// A read-only application layout bound to the view that admitted its source.
///
/// The name plan retains every source entry and its kind. A copy executor must
/// still qualify forks, attributes, links, hard-link groups, and modes, carry
/// quarantine through publication, and verify the staged result.
#[derive(Debug)]
pub struct PlannedApplication<Id> {
	source: NodeHandle<Id>,
	executable: NodeHandle<Id>,
	destination_name: HostName,
	names: NamePlan<Id>,
}

impl<Id> PlannedApplication<Id> {
	#[must_use]
	pub const fn source(&self) -> &NodeHandle<Id> {
		&self.source
	}

	#[must_use]
	pub const fn executable(&self) -> &NodeHandle<Id> {
		&self.executable
	}

	/// The exact on-volume application basename under the chosen name profile.
	#[must_use]
	pub const fn destination_name(&self) -> &HostName {
		&self.destination_name
	}

	#[must_use]
	pub const fn names(&self) -> &NamePlan<Id> {
		&self.names
	}

	/// Checks the pinned destination directory for a fresh output name.
	///
	/// A later copy must recheck attachment and use a no-clobber publication.
	///
	/// # Errors
	/// Returns an error for an existing entry, a moved or replaced directory,
	/// or an inspection failure.
	#[cfg(any(target_os = "linux", target_os = "macos"))]
	pub fn require_fresh_destination(
		&self,
		destination: &StableDestination,
	) -> Result<(), HostError> {
		destination
			.directory()
			.require_absent(&self.destination_name)
	}
}

/// Application selection or preflight failed before opening the destination.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error<E: StdError + 'static> {
	#[error("could not resolve the selected application in the source volume")]
	Source(#[from] ViewError<E>),
	#[error("the application selection exceeds the path budget")]
	SourcePathLimit,
	#[error("the selected path does not exist on the source volume")]
	MissingSource,
	#[error("select an application bundle directory with a .app extension")]
	ExpectedApplication,
	#[error("the selected application's destination name cannot be preserved")]
	DestinationName(#[from] NameError),
	#[error("the application is missing its required {0:?} member")]
	BundleMember(RequiredMember),
	#[error("application Info.plist has no data fork")]
	MissingInfo,
	#[error("application Info.plist exceeds the inspection budget")]
	InfoLimit,
	#[error("could not reserve the application Info.plist inspection buffer")]
	Reserve(#[from] TryReserveError),
	#[error("application Info.plist does not declare an unambiguous executable name")]
	Info,
	#[error("the declared application executable needs executable mode bits and a data fork")]
	Executable,
	#[error(transparent)]
	Names(#[from] NamePlanError<E>),
	#[error("application planning was cancelled")]
	Cancelled,
}

/// Admits one explicit bundle and plans its names without following links.
///
/// The selected profile must describe the intended destination's name
/// equivalence. Name limits bound relative paths and entries below the bundle
/// root. The bundle name itself is also checked against the component limit.
/// Only the bounded Info.plist is read. The returned plan establishes bundle
/// layout and source identity for further copy qualification.
///
/// # Errors
/// Rejects invalid or unavailable source paths, incompatible layout or names,
/// unverifiable bundle members, exceeded limits, and cancellation.
pub fn prepare_application<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	source: &VirtualPath,
	profile: NameProfile,
	limits: NameLimits,
	mut cancelled: impl FnMut() -> bool,
) -> Result<PlannedApplication<B::Id>, Error<B::Error>>
where
	B::Id: Eq + Hash,
{
	let selected = resolve(view, source, limits, &mut cancelled)?;
	let destination_name = HostName::admit(selected.name, profile, limits.max_component_bytes)?;
	if selected.kind != NodeKind::Directory || !is_application_name(destination_name.as_str()) {
		return Err(Error::ExpectedApplication);
	}
	let names = preflight_names(view, &selected.node, profile, limits, &mut cancelled)?;
	let executable = validate_layout(view, &names, limits.max_component_bytes, &mut cancelled)?;
	Ok(PlannedApplication {
		source: selected.node,
		executable,
		destination_name,
		names,
	})
}

fn is_application_name(name: &str) -> bool {
	name.rsplit_once(EXTENSION_SEPARATOR)
		.is_some_and(|(stem, extension)| {
			!stem.is_empty() && extension.eq_ignore_ascii_case(APPLICATION_EXTENSION)
		})
}

fn resolve<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	source: &VirtualPath,
	limits: NameLimits,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<ViewEntry<B::Id>, Error<B::Error>> {
	if source.is_root() {
		return Err(Error::ExpectedApplication);
	}
	if source.as_str().len() - 1 > limits.max_relative_path_bytes {
		return Err(Error::SourcePathLimit);
	}
	check_cancellation(cancelled)?;
	let mut node = view.root()?;
	let mut components = source.components().enumerate().peekable();
	while let Some((depth, part)) = components.next() {
		check_cancellation(cancelled)?;
		if depth >= limits.max_depth || part.len() > limits.max_component_bytes {
			return Err(Error::SourcePathLimit);
		}
		let entry = view
			.lookup(&node, LookupName::Utf8(part))?
			.ok_or(Error::MissingSource)?;
		if components.peek().is_none() {
			return Ok(entry);
		}
		node = entry.node;
	}
	Err(Error::ExpectedApplication)
}

fn validate_layout<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	names: &NamePlan<B::Id>,
	max_name_bytes: usize,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<NodeHandle<B::Id>, Error<B::Error>> {
	let (contents, _) = member(names, None, CONTENTS_NAME, NodeKind::Directory)
		.ok_or(Error::BundleMember(RequiredMember::Contents))?;
	let (_, plist) = member(
		names,
		Some(contents),
		INFO_PLIST_NAME,
		NodeKind::RegularFile,
	)
	.ok_or(Error::BundleMember(RequiredMember::InfoPlist))?;
	check_cancellation(cancelled)?;
	let bytes = read_info(view, plist)?;
	let executable_name = bundle::executable(&bytes, cancelled)?;
	let executable_name = HostName::new(&executable_name, names.profile(), max_name_bytes)
		.map_err(|_| Error::Info)?;
	let (directory, _) = member(
		names,
		Some(contents),
		EXECUTABLE_DIRECTORY_NAME,
		NodeKind::Directory,
	)
	.ok_or(Error::BundleMember(RequiredMember::ExecutableDirectory))?;
	let (_, executable) = member(
		names,
		Some(directory),
		executable_name.as_str(),
		NodeKind::RegularFile,
	)
	.ok_or(Error::BundleMember(RequiredMember::MainExecutable))?;
	check_cancellation(cancelled)?;
	if view.stat(executable)?.mode & EXECUTE_MODE_BITS == 0
		|| !view
			.with_fork(executable, ForkKind::Data, |fork| !fork.is_empty())?
			.unwrap_or(false)
	{
		return Err(Error::Executable);
	}
	Ok(executable.clone())
}

fn member<'plan, Id>(
	names: &'plan NamePlan<Id>,
	parent: Option<usize>,
	name: &str,
	kind: NodeKind,
) -> Option<(usize, &'plan NodeHandle<Id>)> {
	names
		.entries()
		.iter()
		.enumerate()
		.find_map(|(index, entry)| {
			(entry.parent.map(EntryId::index) == parent
				&& entry.kind == kind
				&& entry.name.as_str() == name)
				.then_some((index, &entry.source))
		})
}

fn read_info<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	info: &NodeHandle<B::Id>,
) -> Result<Vec<u8>, Error<B::Error>> {
	view.with_fork(info, ForkKind::Data, |fork| {
		let length = usize::try_from(fork.len())
			.ok()
			.filter(|&length| length <= MAX_INFO_BYTES)
			.ok_or(Error::InfoLimit)?;
		let mut bytes = Vec::new();
		bytes.try_reserve_exact(length)?;
		bytes.resize(length, 0);
		fork.read_exact_at(0, &mut bytes)?;
		Ok(bytes)
	})?
	.ok_or(Error::MissingInfo)?
}

fn check_cancellation<E: StdError + 'static>(
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error<E>> {
	if cancelled() {
		return Err(Error::Cancelled);
	}
	Ok(())
}
