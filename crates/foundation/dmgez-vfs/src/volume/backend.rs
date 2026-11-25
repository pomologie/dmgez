use std::{
	error::Error as StdError,
	num::{NonZeroU16, NonZeroU32},
};

use super::{
	DirectoryEntry, DirectoryPage, FileDataRanges, FinderInfo, ForkKind, ForkReader, LookupName,
	Metadata, NativeNameKey, NativeNameProfile, Quarantine, QuarantineScope, RawName,
};

/// Engine operator that advances one bounded directory scan.
pub trait DirectoryReader {
	type Id: Copy;
	type Error: StdError + 'static;

	/// Returns at most `limit` entries. Engine scan budgets also apply.
	///
	/// # Errors
	/// Returns the cause of a failed page and retains the prior cursor position.
	fn next_page(&mut self, limit: NonZeroU16) -> Result<DirectoryPage<Self::Id>, Self::Error>;
}

/// Filesystem engine adapter bound to one immutable volume and source.
/// Operations must preserve the source identity and enforce engine I/O budgets.
/// The engine interprets native names, on-disk object kinds, and fork formats.
pub trait VolumeBackend {
	type Id: Copy;
	type Error: StdError + 'static;
	type Directory<'view>: DirectoryReader<Id = Self::Id, Error = Self::Error>
	where
		Self: 'view;

	/// Resolves the visible root directory, validating it as necessary.
	fn root(&mut self) -> Result<Self::Id, Self::Error>;

	/// A numeric engine-native identity, when its identifier has such a representation.
	fn numeric_id(&self, _node: Self::Id) -> Option<u64> {
		None
	}

	/// Queries source quarantine without opening the object's data fork.
	fn quarantine(
		&mut self,
		_node: Self::Id,
		_scope: QuarantineScope,
	) -> Result<Quarantine, Self::Error> {
		Ok(Quarantine::Unavailable)
	}

	/// Finds one child in a directory without resolving a symbolic link.
	fn lookup(
		&mut self,
		parent: Self::Id,
		name: LookupName<'_>,
	) -> Result<Option<DirectoryEntry<Self::Id>>, Self::Error>;

	/// Returns the engine's supported native equality profile for this volume.
	/// The default reports that native comparison is unavailable.
	fn native_name_profile(&self) -> Option<NativeNameProfile> {
		None
	}

	/// Computes a key from one component in its stored spelling.
	///
	/// Keys from volumes with the same supported profile must compare equal
	/// exactly when their admitted stored names are equivalent under that
	/// profile. A UTF-8 input may be the losslessly decoded stored name.
	/// Return `None` when native comparison is unavailable and return an
	/// error if the selected profile cannot key the supplied name.
	fn native_name_key(&self, _name: LookupName<'_>) -> Result<Option<NativeNameKey>, Self::Error> {
		Ok(None)
	}

	/// Gets effective metadata. Hard-link storage keys are local to this view.
	/// Handles sharing a key have the same effective fork presence and bytes.
	fn stat(&mut self, node: Self::Id) -> Result<Option<Metadata<u64>>, Self::Error>;

	/// Reads effective FinderInfo from bounded metadata without opening data forks.
	/// Symbolic link handles address the link itself. Hard links use effective
	/// shared metadata. Return `Absent` only when the backend establishes absence.
	fn finder_info(&mut self, _node: Self::Id) -> Result<FinderInfo, Self::Error> {
		Ok(FinderInfo::Unavailable)
	}

	/// Opens a scan over the selected directory's visible children.
	fn read_dir(&mut self, directory: Self::Id) -> Result<Self::Directory<'_>, Self::Error>;

	/// Lends exactly the selected fork to the callback for this operation.
	/// A `None` result means the fork is absent. A present empty fork lends a
	/// zero-length reader. This operation never follows symbolic links.
	fn with_fork<T>(
		&mut self,
		node: Self::Id,
		kind: ForkKind,
		read: impl FnOnce(&mut dyn ForkReader<Error = Self::Error>) -> T,
	) -> Result<Option<T>, Self::Error>;

	/// Observes stored byte ranges for the regular file's logical data stream.
	/// Ranges are relative to the admitted volume or container, and omit holes and slack.
	fn file_data_ranges(
		&mut self,
		_node: Self::Id,
		_limit: NonZeroU16,
	) -> Result<FileDataRanges, Self::Error> {
		Ok(FileDataRanges::Unavailable)
	}

	/// Lists all exact stored extended-attribute names without reading their values.
	/// Returns `None` when enumeration is unavailable. Rejects an over-limit
	/// listing instead of returning an incomplete result.
	fn list_xattrs(
		&mut self,
		_node: Self::Id,
		_limit: NonZeroU32,
	) -> Result<Option<Vec<RawName>>, Self::Error> {
		Ok(None)
	}

	/// Lends the named attribute's exact stored bytes without following links.
	/// Attribute enumeration establishes whether this operation is supported.
	/// A missing attribute returns `None`; a present empty attribute lends a reader.
	fn with_xattr<T>(
		&mut self,
		_node: Self::Id,
		_name: LookupName<'_>,
		_read: impl FnOnce(&mut dyn ForkReader<Error = Self::Error>) -> T,
	) -> Result<Option<T>, Self::Error> {
		Ok(None)
	}

	/// Returns the exact symbolic link payload without opening its target.
	fn read_link(&mut self, node: Self::Id) -> Result<Option<Vec<u8>>, Self::Error>;
}
