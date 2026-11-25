use std::{error::Error as StdError, fs::File};

use dmgez_vfs::volume::{ForkKind, NodeHandle, VolumeBackend};
use rustix::{
	fd::AsFd,
	fs::{self, AtFlags, Dir, Mode},
};

use crate::install::CopyPlan;

use super::{
	CopyContent, CopyEntry, DestinationDirectory, FINDER_ATTRIBUTE, InstallFailure,
	PROVENANCE_ATTRIBUTE, READ_FILE, RESOURCE_ATTRIBUTE, Run, host_mode, os, read_value, same_file,
	verify_attribute, verify_link,
};

const CURRENT_DIRECTORY: &[u8] = b".";
const PARENT_DIRECTORY: &[u8] = b"..";
const ATTRIBUTE_TERMINATOR: u8 = 0;

impl<B: VolumeBackend> Run<'_, '_, B> {
	pub(in crate::install::execute) fn verify_tree(
		&mut self,
		plan: &CopyPlan<B::Id>,
		children: &[Vec<usize>],
		root: &DestinationDirectory<'_>,
	) -> Result<(), InstallFailure<B::Error>> {
		self.verify_directory(plan, children, plan.entries().len(), root)
	}

	fn verify_directory(
		&mut self,
		plan: &CopyPlan<B::Id>,
		children: &[Vec<usize>],
		index: usize,
		directory: &DestinationDirectory<'_>,
	) -> Result<(), InstallFailure<B::Error>> {
		self.check_ready()?;
		directory.check_attachment()?;
		let names = plan.application().names().entries();
		let (source, entry) = if index == plan.entries().len() {
			(plan.application().source(), plan.root())
		} else {
			(&names[index].source, &plan.entries()[index])
		};
		let fd = directory.descriptor();
		verify_mode(fd, entry.mode)?;
		self.verify_attributes(source, entry, fd)?;
		let mut expected = Vec::new();
		expected.try_reserve_exact(children[index].len())?;
		expected.extend(
			children[index]
				.iter()
				.map(|&child| names[child].name.as_str().as_bytes()),
		);
		verify_directory_names(fd, expected)?;
		for &child in &children[index] {
			self.check_ready()?;
			directory.check_attachment()?;
			let item = &names[child];
			let node = &plan.entries()[child];
			let name = item.name.as_str();
			match &node.content {
				CopyContent::Directory => {
					let scoped = directory.open_child(&item.name)?;
					self.verify_directory(plan, children, child, &scoped)?;
				}
				CopyContent::File { data_bytes, .. } => {
					let mut file = File::from(os(fs::openat(fd, name, READ_FILE, Mode::empty()))?);
					let stat = os(fs::fstat(&file))?;
					let named = os(fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW))?;
					if !same_file(&stat, &named, *data_bytes)
						|| stat.st_dev != os(fs::fstat(fd))?.st_dev
					{
						return Err(InstallFailure::ReadBack);
					}
					verify_mode(&file, node.mode)?;
					self.verify_data(&item.source, *data_bytes, &mut file)?;
					self.verify_attributes(&item.source, node, &file)?;
					let named = os(fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW))?;
					if !same_file(&stat, &named, *data_bytes) {
						return Err(InstallFailure::ReadBack);
					}
				}
				CopyContent::Link(target) => {
					verify_link(fd, name, target, node.mode)?;
					let path = directory.attached_path()?.join(name);
					match_attribute_names(Vec::new(), fs::llistxattr_query(&path), |names| {
						fs::llistxattr(&path, names)
					})?;
				}
			}
		}
		directory.check_attachment()?;
		Ok(())
	}

	fn verify_attributes(
		&mut self,
		source: &NodeHandle<B::Id>,
		entry: &CopyEntry,
		fd: impl AsFd,
	) -> Result<(), InstallFailure<B::Error>> {
		let fd = fd.as_fd();
		verify_attribute_names(fd, entry)?;
		if let Some(value) = &entry.finder_info {
			verify_attribute(fd, FINDER_ATTRIBUTE, value)?;
		}
		if let CopyContent::File {
			resource_bytes: Some(bytes),
			..
		} = entry.content
		{
			let value = self
				.view
				.with_fork(source, ForkKind::Resource, |fork| {
					read_value(fork, bytes, self.cancelled)
				})?
				.ok_or(InstallFailure::SourceChanged)??;
			verify_attribute(fd, RESOURCE_ATTRIBUTE, &value)?;
		}
		for item in &entry.attributes {
			self.check_ready()?;
			let value = self
				.view
				.with_xattr(source, item.source_name.as_lookup(), |fork| {
					read_value(fork, item.bytes, self.cancelled)
				})?
				.ok_or(InstallFailure::SourceChanged)??;
			verify_attribute(fd, &item.host_name, &value)?;
		}
		Ok(())
	}
}

fn verify_directory_names<E: StdError + 'static>(
	fd: impl AsFd,
	mut expected: Vec<&[u8]>,
) -> Result<(), InstallFailure<E>> {
	expected.sort_unstable();
	let mut count = 0;
	for item in os(Dir::read_from(fd))? {
		let item = os(item)?;
		let name = item.file_name().to_bytes();
		if name == CURRENT_DIRECTORY || name == PARENT_DIRECTORY {
			continue;
		}
		if count == expected.len() || expected.binary_search(&name).is_err() {
			return Err(InstallFailure::ReadBack);
		}
		count += 1;
	}
	if count != expected.len() {
		return Err(InstallFailure::ReadBack);
	}
	Ok(())
}

fn verify_mode<E: StdError + 'static>(fd: impl AsFd, mode: u16) -> Result<(), InstallFailure<E>> {
	if Mode::from_raw_mode(os(fs::fstat(fd))?.st_mode) != host_mode(mode) {
		return Err(InstallFailure::ReadBack);
	}
	Ok(())
}

fn verify_attribute_names<E: StdError + 'static>(
	fd: impl AsFd,
	entry: &CopyEntry,
) -> Result<(), InstallFailure<E>> {
	let fd = fd.as_fd();
	let resource = matches!(
		entry.content,
		CopyContent::File {
			resource_bytes: Some(_),
			..
		}
	);
	let mut expected = Vec::new();
	expected.try_reserve_exact(
		entry.attributes.len() + usize::from(entry.finder_info.is_some()) + usize::from(resource),
	)?;
	expected.extend(
		entry
			.attributes
			.iter()
			.map(|attr| attr.host_name.as_bytes()),
	);
	if entry.finder_info.is_some() {
		expected.push(FINDER_ATTRIBUTE.as_bytes());
	}
	if resource {
		expected.push(RESOURCE_ATTRIBUTE.as_bytes());
	}
	match_attribute_names(expected, fs::flistxattr_query(fd), |names| {
		fs::flistxattr(fd, names)
	})
}

fn match_attribute_names<E: StdError + 'static>(
	mut expected: Vec<&[u8]>,
	length: rustix::io::Result<usize>,
	read_names: impl FnOnce(&mut [u8]) -> rustix::io::Result<usize>,
) -> Result<(), InstallFailure<E>> {
	let expected_length: usize = expected
		.iter()
		.map(|name| name.len() + size_of::<u8>())
		.sum();
	let length = os(length)?;
	// macOS may attach this kernel-managed attribute even to a freshly created bundle.
	// The copy profile rejects it as input and neither sets nor removes it.
	let mut host_provenance = cfg!(target_os = "macos")
		&& length == expected_length + PROVENANCE_ATTRIBUTE.len() + size_of::<u8>();
	if length != expected_length && !host_provenance {
		return Err(InstallFailure::ReadBack);
	}
	if length == 0 {
		return Ok(());
	}
	let mut actual = Vec::new();
	actual.try_reserve_exact(length)?;
	actual.resize(length, 0);
	if os(read_names(actual.as_mut_slice()))? != length {
		return Err(InstallFailure::ReadBack);
	}
	let [names @ .., ATTRIBUTE_TERMINATOR] = actual.as_slice() else {
		return Err(InstallFailure::ReadBack);
	};
	for name in names.split(|&byte| byte == ATTRIBUTE_TERMINATOR) {
		if host_provenance && name == PROVENANCE_ATTRIBUTE.as_bytes() {
			host_provenance = false;
			continue;
		}
		let index = expected
			.iter()
			.position(|part| *part == name)
			.ok_or(InstallFailure::ReadBack)?;
		expected.swap_remove(index);
	}
	if !expected.is_empty() || host_provenance {
		return Err(InstallFailure::ReadBack);
	}
	Ok(())
}
