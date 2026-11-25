use std::io::Read;

use dmgez_builder::manifest::{Entry, EntryKind, Manifest, SourceSnapshot};

use super::{Budget, BundleInfoProblem, Error, Evidence, Limits, check_cancellation, paths};

mod info_plist;

const INFO_PLIST_SUFFIX: &str = "/Contents/Info.plist";
const EXECUTABLE_DIRECTORY_SUFFIX: &str = "/Contents/MacOS/";
const POSIX_EXECUTE_BITS: u16 = 0o111;

pub(super) fn check(
	manifest: &Manifest,
	preserve_permissions: bool,
	limits: Limits,
	mut emit: impl FnMut(Evidence) -> Result<(), Error>,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
	let mut remaining = limits.max_total_plist_bytes;
	for entry in manifest.entries() {
		check_cancellation(cancelled)?;
		if !paths::is_app_bundle(entry) {
			continue;
		}
		let bundle = entry.destination.as_str();
		let plist_path = format!("{bundle}{INFO_PLIST_SUFFIX}");
		let Some(plist) = find(manifest, &plist_path) else {
			emit(Evidence::BundleInfoMissing {
				bundle: bundle.to_owned(),
				destination: plist_path,
			})?;
			continue;
		};
		let inspected = inspect_info(plist, limits, &mut remaining, cancelled)?;
		let executable = match inspected {
			Ok(executable) => executable,
			Err(reason) => {
				emit(Evidence::BundleInfoInvalid {
					bundle: bundle.to_owned(),
					destination: plist_path,
					reason,
				})?;
				continue;
			}
		};
		let executable_path = format!("{bundle}{EXECUTABLE_DIRECTORY_SUFFIX}{executable}");
		if let Some(evidence) =
			check_executable(manifest, preserve_permissions, bundle, executable_path)
		{
			emit(evidence)?;
		}
	}
	Ok(())
}

fn find<'manifest>(manifest: &'manifest Manifest, path: &str) -> Option<&'manifest Entry> {
	let entries = manifest.entries();
	let index = entries
		.binary_search_by(|entry| entry.destination.as_str().cmp(path))
		.ok()?;
	entries.get(index)
}

fn inspect_info(
	entry: &Entry,
	limits: Limits,
	remaining: &mut u64,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<Result<String, BundleInfoProblem>, Error> {
	if !matches!(entry.kind, EntryKind::File) {
		return Ok(Err(BundleInfoProblem::NotRegularFile));
	}
	let Some(source) = &entry.source else {
		return Ok(Err(BundleInfoProblem::MissingSource));
	};
	let bytes = read_info(entry.destination.as_str(), source, limits, remaining)?;
	info_plist::executable(&bytes, cancelled)
}

fn read_info(
	destination: &str,
	source: &SourceSnapshot,
	limits: Limits,
	remaining: &mut u64,
) -> Result<Vec<u8>, Error> {
	let size = source.metadata().size;
	let file_limit = || Error::Limit {
		budget: Budget::PlistBytes,
		limit: limits.max_plist_bytes,
	};
	if size > limits.max_plist_bytes {
		return Err(file_limit());
	}
	*remaining = remaining.checked_sub(size).ok_or(Error::Limit {
		budget: Budget::TotalPlistBytes,
		limit: limits.max_total_plist_bytes,
	})?;
	let length = usize::try_from(size).map_err(|_| file_limit())?;
	let mut file = source.open_regular()?;
	let mut bytes = Vec::new();
	bytes.try_reserve_exact(length)?;
	bytes.resize(length, 0);
	file.read_exact(&mut bytes)
		.map_err(|source| Error::SourceIo {
			destination: destination.to_owned(),
			source,
		})?;
	source.validate()?;
	Ok(bytes)
}

fn check_executable(
	manifest: &Manifest,
	preserve_permissions: bool,
	bundle: &str,
	destination: String,
) -> Option<Evidence> {
	let Some(entry) = find(manifest, &destination) else {
		return Some(Evidence::BundleExecutableMissing {
			bundle: bundle.to_owned(),
			destination,
		});
	};
	if !matches!(entry.kind, EntryKind::File) {
		return Some(Evidence::BundleExecutableWrongKind {
			bundle: bundle.to_owned(),
			destination,
		});
	}
	if !preserve_permissions {
		return Some(Evidence::BundleExecutablePermissionsNotPreserved {
			bundle: bundle.to_owned(),
			destination,
		});
	}
	match entry
		.source
		.as_ref()
		.and_then(|source| source.metadata().permissions)
	{
		Some(mode) if mode & POSIX_EXECUTE_BITS == 0 => {
			Some(Evidence::BundleExecutableNotExecutable {
				bundle: bundle.to_owned(),
				destination,
				mode,
			})
		}
		Some(_) => None,
		None => Some(Evidence::BundleExecutableModeUnavailable {
			bundle: bundle.to_owned(),
			destination,
		}),
	}
}
