//! Display flags in a 32-byte catalog Finder information record.

use std::{mem::size_of, ops::Range};

pub(super) const FINDER_INFO_BYTES: usize = 32;
#[cfg(feature = "volume-icon")]
pub(super) const HAS_CUSTOM_ICON: u16 = 0x0400;
pub(super) const INVISIBLE: u16 = 0x4000;
// Foundation's hasHiddenExtension property uses this bit in the ordinary Finder flags.
const HIDE_EXTENSION: u16 = 0x0010;
const FLAGS_OFFSET: usize = 8;
const FLAGS: Range<usize> = FLAGS_OFFSET..FLAGS_OFFSET + size_of::<u16>();

/// Explicit display flags to apply while retaining unspecified Finder metadata.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DisplayFlags {
	/// The Finder invisible flag. This is independent of the filename.
	pub hidden: Option<bool>,
	/// Whether Finder should hide the filename extension.
	pub hide_extension: Option<bool>,
}

impl DisplayFlags {
	/// Reads both ordinary Finder display bits from a stored FinderInfo value.
	#[must_use]
	pub fn read(finder_info: &[u8; FINDER_INFO_BYTES]) -> Self {
		let flags = read_flags(finder_info);
		Self {
			hidden: Some(flags & INVISIBLE != 0),
			hide_extension: Some(flags & HIDE_EXTENSION != 0),
		}
	}

	/// Set or clear the selected bits in the ordinary Finder flags word.
	pub fn apply(self, finder_info: &mut [u8; FINDER_INFO_BYTES]) {
		update_flags(finder_info, |mut flags| {
			for (value, mask) in [
				(self.hidden, INVISIBLE),
				(self.hide_extension, HIDE_EXTENSION),
			] {
				match value {
					Some(true) => flags |= mask,
					Some(false) => flags &= !mask,
					None => {}
				}
			}
			flags
		});
	}
}

#[cfg(feature = "volume-icon")]
pub(super) fn add_flags(finder_info: &mut [u8; FINDER_INFO_BYTES], set: u16) {
	update_flags(finder_info, |flags| flags | set);
}

fn update_flags(finder_info: &mut [u8; FINDER_INFO_BYTES], update: impl FnOnce(u16) -> u16) {
	let current = read_flags(finder_info);
	finder_info[FLAGS].copy_from_slice(&update(current).to_be_bytes());
}

fn read_flags(finder_info: &[u8; FINDER_INFO_BYTES]) -> u16 {
	let flags = &finder_info[FLAGS];
	u16::from_be_bytes([flags[0], flags[1]])
}
