use std::{mem::size_of, ops::Range};

pub(super) const FINDER_INFO_BYTES: usize = 32;
pub(super) const HAS_CUSTOM_ICON: u16 = 0x0400;
pub(super) const INVISIBLE: u16 = 0x4000;
const FLAGS_OFFSET: usize = 8;
const FLAGS: Range<usize> = FLAGS_OFFSET..FLAGS_OFFSET + size_of::<u16>();

pub(super) fn add_flags(finder_info: &mut [u8; FINDER_INFO_BYTES], set: u16) {
	let flags = &mut finder_info[FLAGS];
	let current = u16::from_be_bytes([flags[0], flags[1]]);
	flags.copy_from_slice(&(current | set).to_be_bytes());
}
