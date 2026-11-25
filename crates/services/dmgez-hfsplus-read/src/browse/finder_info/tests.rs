use dmgez_vfs::volume::{FinderInfo, NodeKind};

use super::{EMPTY_FINDER_INFO, macos_finder_info};

#[test]
fn regular_and_directory_finder_info_preserve_visible_bytes() {
	let stored = *b"ABCDEFGHIJKLMNOPQRSTUVWXabcdef12";
	let visible = *b"ABCDEFGHIJKLMNOP\0\0\0\0\0\0\0\0abcd\0\0\0\0";
	for kind in [NodeKind::RegularFile, NodeKind::Directory] {
		assert_eq!(
			macos_finder_info(kind, stored),
			FinderInfo::Present(visible)
		);
	}
	assert_eq!(
		macos_finder_info(NodeKind::Other, stored),
		FinderInfo::Present(stored)
	);
}

#[test]
fn symlink_finder_info_omits_type_and_creator_and_detects_absence() {
	let stored = *b"slnkrhapIJKLMNOPQRSTUVWXabcdef12";
	let visible = *b"\0\0\0\0\0\0\0\0IJKLMNOP\0\0\0\0\0\0\0\0abcd\0\0\0\0";
	assert_eq!(
		macos_finder_info(NodeKind::Symlink, stored),
		FinderInfo::Present(visible)
	);

	let private_only = *b"slnkrhap\0\0\0\0\0\0\0\0QRSTUVWX\0\0\0\0yz12";
	assert_eq!(
		macos_finder_info(NodeKind::Symlink, private_only),
		FinderInfo::Absent
	);
	assert_eq!(
		macos_finder_info(NodeKind::RegularFile, EMPTY_FINDER_INFO),
		FinderInfo::Absent
	);
}
