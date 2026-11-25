use std::{collections::BTreeMap, fs, io, io::Read, os::unix::fs::PermissionsExt, path::Path};

use dmgez_builder::{
	manifest::{Entry, EntryKind, Manifest, SourceSnapshot},
	planning::PreparedBuild,
};
use dmgez_config::VirtualPath;
use dmgez_operations::{
	extract::host::StableDestination,
	install::{
		CopyContent, CopyEntry, CopyLimits, CopyPlan, execute_copy, prepare_copy,
		prepare_staged_application,
	},
};
use dmgez_signing::{
	Backend, ResolvedBackend,
	release_plan::{ImageIdentity, ReleaseError},
	verify_application,
};
use dmgez_vfs::volume::{ForkKind, LookupName, NodeHandle, RawName, VolumeBackend, VolumeView};

use crate::{project_release::StageResult, volume::with_hfs_file_verified};

use super::{artifact, native::NativeReleaseError};

mod inventory;
mod source;

#[cfg(test)]
mod tests;

const PATH_SEPARATOR: char = '/';
const PERMISSIONS: u16 = 0o777;
const BUFFER_BYTES: usize = 64 << 10;
const PRIVATE_PREFIX: &str = ".dmgez-verify-app-";
const OWNER_DIRECTORY_ACCESS: u32 = 0o700;

pub(super) fn prepare(
	prepared: &PreparedBuild,
	backend: ResolvedBackend,
) -> StageResult<Vec<VirtualPath>> {
	source::prepare(prepared, |path| check_signature(path, backend))
}

pub(super) fn verify_unsigned(
	path: &Path,
	expected: &ImageIdentity,
	prepared: &PreparedBuild,
	applications: &[VirtualPath],
	backend: ResolvedBackend,
) -> StageResult<usize> {
	verify_staged(path, expected, prepared.manifest(), applications, |path| {
		check_signature(path, backend)
	})
}

fn check_signature(path: &Path, backend: ResolvedBackend) -> StageResult {
	let backend = match backend {
		ResolvedBackend::Native => Backend::Native,
		_ => {
			return Err(NativeReleaseError::Signing(
				dmgez_signing::SignError::BackendUnavailable {
					backend: Backend::Auto,
				},
			)
			.into());
		}
	};
	verify_application(path, backend).map_err(|error| NativeReleaseError::Signing(error).into())
}

fn verify_staged(
	path: &Path,
	expected: &ImageIdentity,
	manifest: &Manifest,
	applications: &[VirtualPath],
	mut verify: impl FnMut(&Path) -> StageResult,
) -> StageResult<usize> {
	let file = artifact::open_regular(path)?;
	let require_identity = || -> StageResult {
		if artifact::observe_file(file.try_clone()?)? != *expected {
			return Err(ReleaseError::ArtifactChanged.into());
		}
		Ok(())
	};
	require_identity()?;
	with_hfs_file_verified(&file, |view, verify_source| -> StageResult {
		inventory::match_applications(view, applications)?;
		for path in applications {
			verify_source()?;
			verify_bundle(view, manifest, path, verify_source, &mut verify)?;
		}
		verify_source()?;
		Ok(())
	})?;
	require_identity()?;
	Ok(applications.len())
}

fn verify_bundle<B: VolumeBackend<Error: Send + Sync>>(
	view: &mut VolumeView<B>,
	manifest: &Manifest,
	path: &VirtualPath,
	verify_source: &dyn Fn() -> io::Result<()>,
	verify: &mut impl FnMut(&Path) -> StageResult,
) -> StageResult
where
	B::Id: Eq + std::hash::Hash,
{
	let root = exact_path(view, path)?;
	let application = prepare_staged_application(view, path, || false)?;
	if *application.source() != root {
		return Err(bundle_mismatch(path));
	}
	let plan = prepare_copy(view, application, CopyLimits::default(), || false)?;
	match_manifest(view, &plan, manifest, path)?;
	let scratch = tempfile::Builder::new().prefix(PRIVATE_PREFIX).tempdir()?;
	let parent = scratch.path().canonicalize()?;
	let destination = StableDestination::open(&parent)?;
	let result = (|| {
		execute_copy(view, &plan, &destination.directory(), verify_source, || {
			false
		})?;
		verify(&parent.join(plan.application().destination_name().as_str()))
	})();
	let cleanup = writable_directories(&parent).and_then(|()| scratch.close());
	match (result, cleanup) {
		(result, Ok(())) => result,
		(result, Err(error)) => {
			let cleanup = format!(
				"could not remove private application verification directory {parent:?}: {error}"
			);
			let message = result
				.err()
				.map_or_else(|| cleanup.clone(), |error| format!("{error}. {cleanup}"));
			Err(io::Error::new(error.kind(), message).into())
		}
	}
}

fn writable_directories(path: &Path) -> io::Result<()> {
	// This is the private temp directory created for one copy. Each recursion
	// checks the entry type without following links before adjusting its mode.
	let metadata = fs::symlink_metadata(path)?;
	if !metadata.is_dir() {
		return Ok(());
	}
	let mut permissions = metadata.permissions();
	let mode = permissions.mode();
	if mode & OWNER_DIRECTORY_ACCESS != OWNER_DIRECTORY_ACCESS {
		permissions.set_mode(mode | OWNER_DIRECTORY_ACCESS);
		fs::set_permissions(path, permissions)?;
	}
	for child in fs::read_dir(path)? {
		let child = child?;
		if child.file_type()?.is_dir() {
			writable_directories(&child.path())?;
		}
	}
	Ok(())
}

fn exact_path<B: VolumeBackend<Error: Send + Sync>>(
	view: &mut VolumeView<B>,
	path: &VirtualPath,
) -> StageResult<NodeHandle<B::Id>> {
	let mut node = view.root()?;
	for name in path.components() {
		let entry = view
			.lookup(&node, LookupName::Utf8(name))?
			.ok_or_else(|| bundle_mismatch(path))?;
		let exact = match &entry.name {
			RawName::Utf8(stored) => stored == name,
			RawName::Utf16(stored) => stored.iter().copied().eq(name.encode_utf16()),
			_ => false,
		};
		if !exact {
			return Err(bundle_mismatch(path));
		}
		node = entry.node;
	}
	Ok(node)
}

fn match_manifest<B: VolumeBackend<Error: Send + Sync>>(
	view: &mut VolumeView<B>,
	plan: &CopyPlan<B::Id>,
	manifest: &Manifest,
	path: &VirtualPath,
) -> StageResult {
	let entries = manifest.entries();
	let root = entries
		.binary_search_by(|entry| entry.destination.cmp(path))
		.ok()
		.map(|index| &entries[index])
		.ok_or_else(|| bundle_mismatch(path))?;
	match_object(view, plan.application().source(), plan.root(), root)?;
	let prefix = format!("{}{PATH_SEPARATOR}", path.as_str());
	let start = entries.partition_point(|entry| entry.destination.as_str() < prefix.as_str());
	let mut expected = BTreeMap::new();
	for entry in entries[start..]
		.iter()
		.take_while(|entry| entry.destination.as_str().starts_with(&prefix))
	{
		if expected.len() == plan.entries().len() {
			return Err(bundle_mismatch(path));
		}
		let (parent, name) = entry
			.destination
			.as_str()
			.rsplit_once(PATH_SEPARATOR)
			.ok_or_else(|| bundle_mismatch(path))?;
		expected.insert((parent, name), entry);
	}
	if expected.len() != plan.entries().len() {
		return Err(bundle_mismatch(path));
	}
	let mut paths: Vec<&str> = Vec::new();
	paths.try_reserve_exact(plan.entries().len())?;
	for (node, item) in plan
		.application()
		.names()
		.entries()
		.iter()
		.zip(plan.entries())
	{
		let parent = node
			.parent
			.map_or(path.as_str(), |parent| paths[parent.index()]);
		let entry = expected
			.remove(&(parent, node.name.as_str()))
			.ok_or_else(|| bundle_mismatch(path))?;
		match_object(view, &node.source, item, entry)?;
		paths.push(entry.destination.as_str());
	}
	Ok(())
}

fn match_object<B: VolumeBackend<Error: Send + Sync>>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	actual: &CopyEntry,
	expected: &Entry,
) -> StageResult {
	let snapshot = expected
		.source
		.as_ref()
		.ok_or_else(|| bundle_mismatch(&expected.destination))?;
	if snapshot
		.metadata()
		.permissions
		.map(|mode| mode & PERMISSIONS)
		!= Some(actual.mode)
	{
		return Err(bundle_mismatch(&expected.destination));
	}
	snapshot.validate()?;
	match (&actual.content, &expected.kind) {
		(CopyContent::Directory, EntryKind::Directory) => (),
		(CopyContent::Link(actual), EntryKind::Symlink(expected)) if actual == expected => (),
		(CopyContent::File { data_bytes, .. }, EntryKind::File)
			if *data_bytes == snapshot.metadata().size && match_data(view, node, snapshot)? => {}
		_ => return Err(bundle_mismatch(&expected.destination)),
	}
	Ok(())
}

fn match_data<B: VolumeBackend<Error: Send + Sync>>(
	view: &mut VolumeView<B>,
	node: &NodeHandle<B::Id>,
	snapshot: &SourceSnapshot,
) -> StageResult<bool> {
	let mut source = snapshot.open_regular()?;
	let matches = view.with_fork(node, ForkKind::Data, |fork| -> StageResult<bool> {
		if fork.len() != snapshot.metadata().size {
			return Ok(false);
		}
		let mut image = [0; BUFFER_BYTES];
		let mut host = [0; BUFFER_BYTES];
		let mut offset = 0;
		while offset < fork.len() {
			let count = (fork.len() - offset).min(BUFFER_BYTES as u64) as usize;
			fork.read_exact_at(offset, &mut image[..count])?;
			source.read_exact(&mut host[..count])?;
			if image[..count] != host[..count] {
				return Ok(false);
			}
			offset += count as u64;
		}
		Ok(source.read(&mut [0])? == 0)
	})?;
	snapshot.validate()?;
	Ok(matches.transpose()?.unwrap_or(false))
}

fn bundle_mismatch(path: &VirtualPath) -> Box<dyn std::error::Error + Send + Sync> {
	io::Error::new(
		io::ErrorKind::InvalidData,
		format!(
			"staged application does not match the captured manifest at {:?}",
			path.as_str()
		),
	)
	.into()
}
