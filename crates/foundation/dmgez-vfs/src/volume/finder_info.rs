/// Byte length of the macOS FinderInfo value, including extended Finder info.
pub const FINDER_INFO_BYTES: usize = 32;

/// Effective per-object FinderInfo from the selected filesystem.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FinderInfo {
	/// The backend cannot establish the object's FinderInfo state.
	Unavailable,
	/// The backend established that the object has no effective FinderInfo.
	Absent,
	/// The backend read exactly this effective FinderInfo value.
	Present([u8; FINDER_INFO_BYTES]),
}
