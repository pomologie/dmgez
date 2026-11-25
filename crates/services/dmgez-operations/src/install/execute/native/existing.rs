use std::{
	collections::TryReserveError, convert::Infallible, error::Error as StdError, fmt, fs::File, io,
};

use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use rustix::fs::{self, FileType, Mode, OFlags, Stat};
use thiserror::Error;

use crate::{
	extract::host::HostError,
	install::{CopyPlan, CopySummary},
};

use super::{
	CopyContent, DestinationDirectory, InstallFailure, READ_FILE, Run, host_mode, os,
	readback::{verify_directory_names, verify_empty_attributes},
	verify_link,
};

mod observation;

pub use observation::{BundleIdentity, BundleStamp, observe_existing_bundle};
use observation::{
	ParentBinding, check_materialized, check_name, check_stamp, kind, named, observe_bundle_name,
	open_bundle,
};

// Darwin opens the link itself with O_SYMLINK, including a dangling link.
const READ_LINK: OFlags = OFlags::SYMLINK
	.union(OFlags::RDONLY)
	.union(OFlags::NONBLOCK)
	.union(OFlags::CLOEXEC);

/// A stable, matching application bundle could not be confirmed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExistingBundleError<E: StdError + 'static> {
	#[error("the selected application is absent from the destination")]
	Missing,
	#[error("the exact destination application does not match the copy plan")]
	Mismatch,
	#[error("the destination namespace or application changed during verification")]
	Changed,
	#[error("the destination parent exceeds the exact-name scan limit")]
	ParentLimit,
	#[error("the destination application requires local filesystem storage")]
	NonLocal,
	#[error("the destination application contains a dataless object")]
	Dataless,
	#[error("the bundle evidence belongs to a different copy plan")]
	DifferentPlan,
	#[error("verification of the destination application was cancelled")]
	Cancelled,
	#[error("could not observe the destination application")]
	Io(#[source] io::Error),
	#[error("could not complete destination application verification")]
	Check(#[source] InstallFailure<E>),
}

impl<E: StdError + 'static> From<InstallFailure<E>> for ExistingBundleError<E> {
	fn from(error: InstallFailure<E>) -> Self {
		match error {
			InstallFailure::Cancelled => Self::Cancelled,
			InstallFailure::ReadBack => Self::Mismatch,
			InstallFailure::Host(HostError::Detached) => Self::Changed,
			InstallFailure::Io(error) => Self::Io(error),
			other => Self::Check(other),
		}
	}
}

impl<E: StdError + 'static> From<HostError> for ExistingBundleError<E> {
	fn from(error: HostError) -> Self {
		InstallFailure::Host(error).into()
	}
}

impl<E: StdError + 'static> From<TryReserveError> for ExistingBundleError<E> {
	fn from(error: TryReserveError) -> Self {
		InstallFailure::Reserve(error).into()
	}
}

type Check<T, E> = Result<T, ExistingBundleError<E>>;

/// Compares an existing macOS application bundle with its admitted copy plan.
///
/// `destination` is the held, authorized parent of the plan's exact bundle
/// basename. File data, modes, admitted attributes, and link targets are read
/// relative to held directories. The supplied callback must revalidate the
/// immutable image that backs `view`. The final metadata and namespace pass
/// checks every observed entry for replacement or change. A destination must
/// be local. An object observed as dataless is refused before opening it for
/// verification. This assumes a stable namespace and cannot exclude writes
/// after an observation.
/// The result describes this comparison and supplies no authorization for an
/// install, subsequent use of the bundle, or detachment.
///
/// # Errors
/// Reports an absent, mismatched, changed, or unverifiable bundle, insufficient
/// resources, an invalid source, and cancellation.
pub fn verify_existing_bundle<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	plan: &CopyPlan<B::Id>,
	destination: &DestinationDirectory<'_>,
	verify_source: impl FnMut() -> io::Result<()>,
	cancelled: impl FnMut() -> bool,
) -> Check<CopySummary, B::Error> {
	Ok(
		verify_existing_bundle_observed(view, plan, destination, verify_source, cancelled)?
			.summary(),
	)
}

/// Compares an existing bundle and retains evidence for a later destination recheck.
///
/// This uses the same scope, source checks and bounds as [`verify_existing_bundle`].
///
/// # Errors
/// Returns an error if the comparison fails or its evidence cannot be retained.
pub fn verify_existing_bundle_observed<'plan, B: VolumeBackend>(
	view: &mut VolumeView<B>,
	plan: &'plan CopyPlan<B::Id>,
	destination: &DestinationDirectory<'_>,
	mut verify_source: impl FnMut() -> io::Result<()>,
	mut cancelled: impl FnMut() -> bool,
) -> Check<VerifiedBundle<'plan, B::Id>, B::Error> {
	let mut run = Run {
		view,
		verify_source: &mut verify_source,
		cancelled: &mut cancelled,
	};
	let children = run.prepare(plan)?;
	let parent = ParentBinding::observe(destination)?;
	let name = plan.application().destination_name();
	let root =
		open_bundle(destination, name, run.cancelled)?.ok_or(ExistingBundleError::Missing)?;
	let mut entries = Vec::new();
	entries.try_reserve_exact(plan.entries().len())?;
	entries.resize(plan.entries().len(), root.stamp.0);
	let mut comparison = Comparison {
		plan,
		children,
		entries,
	};
	comparison.compare_directory(
		&mut run,
		&root.directory,
		plan.entries().len(),
		&root.stamp.0,
	)?;
	run.check_ready()?;
	let verified = VerifiedBundle {
		stamp: root.stamp,
		parent,
		comparison,
	};
	verified.recheck_tree(destination, &root.directory, run.cancelled)?;
	run.check_ready()?;
	Ok(verified)
}

/// Observed root and child identities retained by an exact bundle comparison.
pub struct VerifiedBundle<'plan, Id> {
	stamp: BundleStamp,
	parent: ParentBinding,
	comparison: Comparison<'plan, Id>,
}

impl<Id> VerifiedBundle<'_, Id> {
	/// Returns the mutation-sensitive observation of this bundle's root.
	#[must_use]
	pub const fn stamp(&self) -> BundleStamp {
		self.stamp
	}

	/// Returns the inventory summary of the admitted copy plan.
	#[must_use]
	pub const fn summary(&self) -> CopySummary {
		self.comparison.plan.summary()
	}

	/// Rechecks the exact tree relative to the originally observed parent.
	///
	/// Reopen the held source separately if its continued validity is needed.
	/// This reads destination names and change stamps without rereading payloads.
	///
	/// # Errors
	/// Returns an error for a different copy plan, a moved or changed bundle or
	/// parent, an unavailable observation, or cancellation.
	pub fn recheck(
		&self,
		plan: &CopyPlan<Id>,
		destination: &DestinationDirectory<'_>,
		mut cancelled: impl FnMut() -> bool,
	) -> Check<(), Infallible> {
		if !std::ptr::eq(plan, self.comparison.plan) {
			return Err(ExistingBundleError::DifferentPlan);
		}
		check_cancelled(&mut cancelled)?;
		self.parent.recheck(destination)?;
		let current = open_bundle(
			destination,
			plan.application().destination_name(),
			&mut cancelled,
		)
		.map_err(changed_namespace)?
		.ok_or(ExistingBundleError::Changed)?;
		check_stamp(&self.stamp.0, &current.stamp.0)?;
		self.recheck_tree(destination, &current.directory, &mut cancelled)
	}

	fn recheck_tree<E: StdError + 'static>(
		&self,
		destination: &DestinationDirectory<'_>,
		root: &DestinationDirectory<'_>,
		cancelled: &mut dyn FnMut() -> bool,
	) -> Check<(), E> {
		let plan = self.comparison.plan;
		self.comparison
			.recheck_directory(root, plan.entries().len(), &self.stamp.0, cancelled)?;
		let name = plan.application().destination_name();
		let current = observe_bundle_name(destination, name.as_str(), cancelled)
			.map_err(changed_namespace)?;
		if current.map(BundleStamp) != Some(self.stamp) {
			return Err(ExistingBundleError::Changed);
		}
		self.parent.recheck(destination)
	}
}

impl<Id> fmt::Debug for VerifiedBundle<'_, Id> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter
			.debug_struct("VerifiedBundle")
			.finish_non_exhaustive()
	}
}

struct Comparison<'plan, Id> {
	plan: &'plan CopyPlan<Id>,
	children: Vec<Vec<usize>>,
	entries: Vec<Stat>,
}

impl<Id> Comparison<'_, Id> {
	fn compare_directory<B: VolumeBackend<Id = Id>>(
		&mut self,
		run: &mut Run<'_, '_, B>,
		directory: &DestinationDirectory<'_>,
		index: usize,
		initial: &Stat,
	) -> Check<(), B::Error> {
		run.check_ready()?;
		directory.check_attachment()?;
		let fd = directory.descriptor();
		check_stamp(initial, &os(fs::fstat(fd))?)?;
		self.check_names(directory, index)?;
		let plan = self.plan;
		let names = plan.application().names().entries();
		let (source, entry) = if index == plan.entries().len() {
			(plan.application().source(), plan.root())
		} else {
			(&names[index].source, &plan.entries()[index])
		};
		check_mode(initial, entry.mode)?;
		run.verify_attributes(source, entry, fd)?;
		for child_index in 0..self.children[index].len() {
			run.check_ready()?;
			let child = self.children[index][child_index];
			let item = &names[child];
			let name = item.name.as_str();
			let node = &plan.entries()[child];
			let before = named(fd, name)?.ok_or(ExistingBundleError::Changed)?;
			if before.st_dev != initial.st_dev {
				return Err(ExistingBundleError::Mismatch);
			}
			check_materialized(&before)?;
			check_mode(&before, node.mode)?;
			match &node.content {
				CopyContent::Directory if kind(&before) == FileType::Directory => {
					let child_dir = directory.open_child(&item.name)?;
					self.compare_directory(run, &child_dir, child, &before)?;
				}
				CopyContent::File { data_bytes, .. }
					if kind(&before) == FileType::RegularFile
						&& before.st_nlink == 1
						&& u64::try_from(before.st_size).ok() == Some(*data_bytes) =>
				{
					let mut file = File::from(os(fs::openat(fd, name, READ_FILE, Mode::empty()))?);
					check_stamp(&before, &os(fs::fstat(&file))?)?;
					run.verify_data(&item.source, *data_bytes, &mut file)?;
					run.verify_attributes(&item.source, node, &file)?;
					check_stamp(&before, &os(fs::fstat(&file))?)?;
				}
				CopyContent::Link(target) if kind(&before) == FileType::Symlink => {
					let link = os(fs::openat(fd, name, READ_LINK, Mode::empty()))?;
					check_stamp(&before, &os(fs::fstat(&link))?)?;
					verify_link(fd, name, target, node.mode)?;
					verify_empty_attributes(&link)?;
					check_stamp(&before, &os(fs::fstat(&link))?)?;
				}
				_ => return Err(ExistingBundleError::Mismatch),
			}
			check_name(fd, name, &before)?;
			self.entries[child] = before;
		}
		check_stamp(initial, &os(fs::fstat(fd))?)?;
		directory.check_attachment()?;
		Ok(())
	}

	fn recheck_directory<E: StdError + 'static>(
		&self,
		directory: &DestinationDirectory<'_>,
		index: usize,
		initial: &Stat,
		cancelled: &mut dyn FnMut() -> bool,
	) -> Check<(), E> {
		check_cancelled(cancelled)?;
		directory.check_attachment()?;
		let fd = directory.descriptor();
		check_stamp(initial, &os(fs::fstat(fd))?)?;
		self.check_names(directory, index)
			.map_err(changed_namespace)?;
		let names = self.plan.application().names().entries();
		for &child in &self.children[index] {
			check_cancelled(cancelled)?;
			let name = &names[child].name;
			let before = &self.entries[child];
			check_name(fd, name.as_str(), before)?;
			if kind(before) == FileType::Directory {
				let child_dir = directory.open_child(name)?;
				self.recheck_directory(&child_dir, child, before, cancelled)?;
			}
		}
		check_stamp(initial, &os(fs::fstat(fd))?)?;
		directory.check_attachment()?;
		Ok(())
	}

	fn check_names<E: StdError + 'static>(
		&self,
		directory: &DestinationDirectory<'_>,
		index: usize,
	) -> Check<(), E> {
		let names = self.plan.application().names().entries();
		verify_directory_names(
			directory.descriptor(),
			self.children[index]
				.iter()
				.map(|&child| names[child].name.as_str().as_bytes()),
		)?;
		Ok(())
	}
}

fn changed_namespace<E: StdError + 'static>(
	error: ExistingBundleError<E>,
) -> ExistingBundleError<E> {
	match error {
		ExistingBundleError::Mismatch => ExistingBundleError::Changed,
		other => other,
	}
}

fn check_mode<E: StdError + 'static>(stat: &Stat, mode: u16) -> Check<(), E> {
	if Mode::from_raw_mode(stat.st_mode) != host_mode(mode) {
		return Err(ExistingBundleError::Mismatch);
	}
	Ok(())
}

fn check_cancelled<E: StdError + 'static>(cancelled: &mut dyn FnMut() -> bool) -> Check<(), E> {
	if cancelled() {
		return Err(ExistingBundleError::Cancelled);
	}
	Ok(())
}
