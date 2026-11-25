use dmgez_builder::manifest::{Entry, EntryKind};

use super::{Error, Evidence, check_cancellation};

const APPLICATIONS_DESTINATION: &str = "/Applications";
const APPLICATIONS_TARGET: &[u8] = APPLICATIONS_DESTINATION.as_bytes();
const APP_BUNDLE_SUFFIX: &[u8; 4] = b".app";
const POSIX_SEPARATOR: u8 = b'/';
const CURRENT_DIRECTORY: &[u8] = b".";
const PARENT_DIRECTORY: &[u8] = b"..";

pub(super) fn check_paths(
	entries: &[Entry],
	mut emit: impl FnMut(Evidence) -> Result<(), Error>,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
	let mut requires_shortcut = false;
	let mut found_shortcut = false;
	for entry in entries {
		check_cancellation(cancelled)?;
		let destination = entry.destination.as_str();
		if destination == APPLICATIONS_DESTINATION {
			found_shortcut = true;
			let target = match &entry.kind {
				EntryKind::Symlink(target) if target == APPLICATIONS_TARGET => continue,
				EntryKind::Symlink(target) => Some(target.clone()),
				_ => None,
			};
			emit(Evidence::InvalidApplicationsLink {
				destination: destination.to_owned(),
				target,
			})?;
			continue;
		}

		let depth = entry.destination.components().count();
		requires_shortcut |= depth == 1 && is_app_bundle(entry);
		if let EntryKind::Symlink(target) = &entry.kind
			&& escapes_root(target, depth.saturating_sub(1))
		{
			emit(Evidence::ExternalSymlink {
				destination: destination.to_owned(),
				target: target.clone(),
			})?;
		}
	}
	if requires_shortcut && !found_shortcut {
		emit(Evidence::MissingApplicationsLink {
			destination: APPLICATIONS_DESTINATION.to_owned(),
		})?;
	}
	Ok(())
}

pub(super) fn is_app_bundle(entry: &Entry) -> bool {
	matches!(entry.kind, EntryKind::Directory)
		&& entry.destination.components().last().is_some_and(|name| {
			name.as_bytes()
				.last_chunk::<{ APP_BUNDLE_SUFFIX.len() }>()
				.is_some_and(|suffix| suffix.eq_ignore_ascii_case(APP_BUNDLE_SUFFIX))
		})
}

fn escapes_root(target: &[u8], mut depth: usize) -> bool {
	// A mounted installer's links use POSIX target bytes on every lint host.
	if target.first() == Some(&POSIX_SEPARATOR) {
		return true;
	}
	for component in target.split(|byte| *byte == POSIX_SEPARATOR) {
		match component {
			[] | CURRENT_DIRECTORY => {}
			PARENT_DIRECTORY => {
				let Some(parent) = depth.checked_sub(1) else {
					return true;
				};
				depth = parent;
			}
			_ => depth += 1,
		}
	}
	false
}

#[cfg(test)]
mod tests;
