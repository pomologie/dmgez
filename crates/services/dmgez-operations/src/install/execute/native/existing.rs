use std::{collections::TryReserveError, error::Error as StdError, fs::File, io};

use dmgez_vfs::volume::{VolumeBackend, VolumeView};
use rustix::{
	fd::AsFd,
	fs::{self, AtFlags, Dir, FileType, Mode, OFlags, Stat},
	io::Errno,
};
use thiserror::Error;

use crate::{
	extract::host::HostError,
	install::{CopyPlan, CopySummary},
};

use super::{
	CopyContent, DestinationDirectory, InstallFailure, READ_FILE, Run, host_mode, os,
	readback::{
		CURRENT_DIRECTORY, PARENT_DIRECTORY, verify_directory_names, verify_empty_attributes,
	},
	verify_link,
};

const MAX_PARENT_ENTRIES: usize = 16_384;
const MNT_LOCAL: u32 = 0x0000_1000; // Darwin sys/mount.h
const SF_DATALESS: u32 = 0x4000_0000; // Darwin sys/stat.h
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
	mut verify_source: impl FnMut() -> io::Result<()>,
	mut cancelled: impl FnMut() -> bool,
) -> Check<CopySummary, B::Error> {
	let mut run = Run {
		view,
		verify_source: &mut verify_source,
		cancelled: &mut cancelled,
	};
	let children = run.prepare(plan)?;
	if os(fs::fstatfs(destination.descriptor()))?.f_flags & MNT_LOCAL == 0 {
		return Err(ExistingBundleError::NonLocal);
	}
	let name = plan.application().destination_name();
	let initial = observe_bundle_name(destination, name.as_str(), run.cancelled)?;
	let initial = initial.ok_or(ExistingBundleError::Missing)?;
	if kind(&initial) != FileType::Directory {
		return Err(ExistingBundleError::Mismatch);
	}
	check_materialized(&initial)?;
	let root = destination.open_child(name)?;
	let mut entries = Vec::new();
	entries.try_reserve_exact(plan.entries().len())?;
	entries.resize(plan.entries().len(), initial);
	let mut comparison = Comparison {
		plan,
		children,
		entries,
	};
	let index = plan.entries().len();
	comparison.compare_directory(&mut run, &root, index, &initial)?;
	run.check_ready()?;
	comparison.recheck_directory(&root, index, &initial, run.cancelled)?;
	let current = observe_bundle_name(destination, name.as_str(), run.cancelled)
		.map_err(changed_namespace)?;
	if !current.is_some_and(|current| unchanged(&initial, &current)) {
		return Err(ExistingBundleError::Changed);
	}
	run.check_ready()?;
	Ok(plan.summary())
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

fn observe_bundle_name<E: StdError + 'static>(
	parent: &DestinationDirectory<'_>,
	name: &str,
	cancelled: &mut dyn FnMut() -> bool,
) -> Check<Option<Stat>, E> {
	check_cancelled(cancelled)?;
	parent.check_attachment()?;
	let fd = parent.descriptor();
	let stamp = os(fs::fstat(fd))?;
	let initial = named(fd, name)?;
	if let Some(initial) = &initial {
		let mut count = 0;
		let mut exact = false;
		for item in os(Dir::read_from(fd))? {
			check_cancelled(cancelled)?;
			let item = os(item)?;
			let entry = item.file_name().to_bytes();
			if entry == CURRENT_DIRECTORY || entry == PARENT_DIRECTORY {
				continue;
			}
			if count == MAX_PARENT_ENTRIES {
				return Err(ExistingBundleError::ParentLimit);
			}
			if entry == name.as_bytes() {
				exact = true;
				break;
			}
			count += 1;
		}
		check_name(fd, name, initial)?;
		check_stamp(&stamp, &os(fs::fstat(fd))?)?;
		if !exact {
			return Err(ExistingBundleError::Mismatch);
		}
	} else if named(fd, name)?.is_some() {
		return Err(ExistingBundleError::Changed);
	}
	check_stamp(&stamp, &os(fs::fstat(fd))?)?;
	parent.check_attachment()?;
	Ok(initial)
}

fn named<E: StdError + 'static>(fd: impl AsFd, name: &str) -> Check<Option<Stat>, E> {
	match fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW) {
		Ok(stat) => Ok(Some(stat)),
		Err(Errno::NOENT) => Ok(None),
		Err(error) => Err(ExistingBundleError::Io(error.into())),
	}
}

fn check_name<E: StdError + 'static>(fd: impl AsFd, name: &str, before: &Stat) -> Check<(), E> {
	let current = named(fd, name)?.ok_or(ExistingBundleError::Changed)?;
	check_stamp(before, &current)
}

fn check_stamp<E: StdError + 'static>(before: &Stat, current: &Stat) -> Check<(), E> {
	if !unchanged(before, current) {
		return Err(ExistingBundleError::Changed);
	}
	Ok(())
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

fn check_materialized<E: StdError + 'static>(stat: &Stat) -> Check<(), E> {
	if stat.st_flags & SF_DATALESS != 0 {
		return Err(ExistingBundleError::Dataless);
	}
	Ok(())
}

fn check_cancelled<E: StdError + 'static>(cancelled: &mut dyn FnMut() -> bool) -> Check<(), E> {
	if cancelled() {
		return Err(ExistingBundleError::Cancelled);
	}
	Ok(())
}

fn unchanged(before: &Stat, current: &Stat) -> bool {
	before.st_dev == current.st_dev
		&& before.st_ino == current.st_ino
		&& before.st_gen == current.st_gen
		&& before.st_birthtime == current.st_birthtime
		&& before.st_birthtime_nsec == current.st_birthtime_nsec
		&& before.st_ctime == current.st_ctime
		&& before.st_ctime_nsec == current.st_ctime_nsec
		&& before.st_mtime == current.st_mtime
		&& before.st_mtime_nsec == current.st_mtime_nsec
		&& before.st_mode == current.st_mode
		&& before.st_uid == current.st_uid
		&& before.st_gid == current.st_gid
		&& before.st_size == current.st_size
		&& before.st_nlink == current.st_nlink
		&& before.st_flags == current.st_flags
}

fn kind(stat: &Stat) -> FileType {
	FileType::from_raw_mode(stat.st_mode)
}
