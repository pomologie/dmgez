use super::{LookupName, ViewError, VolumeBackend, VolumeView};

/// Native filename equality admitted for one volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NativeNameProfile {
	/// HFS+ Unicode case-folded catalog comparison.
	HfsUnicodeFolded,
	/// HFSX binary comparison of the stored UTF-16 units.
	HfsXBinary,
	/// APFS binary comparison of the stored UTF-8 bytes.
	ApfsBinary,
	/// APFS canonical decomposition of names within the Unicode 9 repertoire.
	ApfsUnicode9Normalized,
	/// APFS canonical decomposition and full case folding within Unicode 9.
	ApfsUnicode9Folded,
}

impl NativeNameProfile {
	/// Stable identifier for reports and diagnostics.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::HfsUnicodeFolded => "hfs_unicode_folded",
			Self::HfsXBinary => "hfsx_binary",
			Self::ApfsBinary => "apfs_binary",
			Self::ApfsUnicode9Normalized => "apfs_unicode9_normalized",
			Self::ApfsUnicode9Folded => "apfs_unicode9_folded",
		}
	}
}

/// Engine-generated equality key for a single stored path component.
///
/// Interpret equality only between keys from the same native name profile.
/// The total order supports indexing independently of native directory order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NativeNameKey(Vec<u8>);

impl NativeNameKey {
	/// Wraps equality bytes computed under the backend's admitted profile.
	#[must_use]
	pub fn new(bytes: Vec<u8>) -> Self {
		Self(bytes)
	}

	/// Borrows the opaque bytes, for example to account for the key's storage.
	#[must_use]
	pub fn as_bytes(&self) -> &[u8] {
		&self.0
	}
}

impl<B: VolumeBackend> VolumeView<B> {
	/// Returns the supported native equality profile for this volume.
	#[must_use]
	pub fn native_name_profile(&self) -> Option<NativeNameProfile> {
		self.backend.native_name_profile()
	}

	/// Computes a native key from one component in its stored spelling.
	/// A `None` result means native comparison is unavailable.
	///
	/// # Errors
	/// Propagates backend failures to key the supplied name.
	pub fn native_name_key(
		&self,
		name: LookupName<'_>,
	) -> Result<Option<NativeNameKey>, ViewError<B::Error>> {
		self.backend
			.native_name_key(name)
			.map_err(ViewError::Backend)
	}
}
