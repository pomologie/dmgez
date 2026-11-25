use std::{collections::TryReserveError, error::Error as StdError, fmt, io};

use dmgez_vfs::volume::{ForkKind, ViewError};

/// A step of a volume query that invokes an engine operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
	Root,
	Lookup { component: usize },
	Directory,
	DirectoryPage,
	Metadata,
	Identity,
	Link,
	OpenFork,
	ReadFork { offset: u64 },
}

impl fmt::Display for Action {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Root => formatter.write_str("resolve the volume root"),
			Self::Lookup { component } => write!(formatter, "resolve path component {component}"),
			Self::Directory => formatter.write_str("open a directory"),
			Self::DirectoryPage => formatter.write_str("read a directory page"),
			Self::Metadata => formatter.write_str("read object metadata"),
			Self::Identity => formatter.write_str("read the object identity"),
			Self::Link => formatter.write_str("read the symbolic-link payload"),
			Self::OpenFork => formatter.write_str("open the selected fork"),
			Self::ReadFork { offset } => {
				write!(formatter, "read the selected fork at byte {offset}")
			}
		}
	}
}

/// Reason a volume path could not be admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidPath {
	Relative,
	TooLong {
		maximum_bytes: usize,
	},
	TooManyComponents {
		maximum: usize,
	},
	InvalidComponent {
		component: usize,
	},
	ComponentTooLong {
		component: usize,
		maximum_bytes: usize,
	},
}

impl fmt::Display for InvalidPath {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Relative => formatter.write_str("volume paths must start at /"),
			Self::TooLong { maximum_bytes } => {
				write!(
					formatter,
					"volume paths may contain at most {maximum_bytes} bytes"
				)
			}
			Self::TooManyComponents { maximum } => {
				write!(
					formatter,
					"volume paths may contain at most {maximum} components"
				)
			}
			Self::InvalidComponent { component } => {
				write!(
					formatter,
					"volume path component {component} is empty, reserved, or contains NUL"
				)
			}
			Self::ComponentTooLong {
				component,
				maximum_bytes,
			} => write!(
				formatter,
				"volume path component {component} may contain at most {maximum_bytes} bytes"
			),
		}
	}
}

impl StdError for InvalidPath {}

/// A failed filesystem query with its engine, output, or admission cause.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
	InvalidPath(InvalidPath),
	MissingPath {
		component: usize,
	},
	ListOffset {
		maximum: u64,
	},
	ListLimit {
		maximum: u16,
	},
	TreeDepth {
		maximum: u16,
	},
	TreeLimit {
		maximum: u32,
	},
	UnsupportedVolume,
	UnsupportedName,
	StalledDirectory,
	ForkAbsent {
		kind: ForkKind,
	},
	ForkLimit {
		length: u64,
		maximum: u64,
	},
	Allocation(TryReserveError),
	Output(io::Error),
	View {
		action: Action,
		source: ViewError<E>,
	},
}

impl<E> Error<E> {
	pub(super) const fn view(action: Action, source: ViewError<E>) -> Self {
		Self::View { action, source }
	}
}

impl<E> fmt::Display for Error<E> {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::InvalidPath(error) => fmt::Display::fmt(error, formatter),
			Self::MissingPath { component } => {
				write!(formatter, "volume path component {component} was not found")
			}
			Self::ListOffset { maximum } => {
				write!(formatter, "list offset may be at most {maximum} entries")
			}
			Self::ListLimit { maximum } => {
				write!(
					formatter,
					"list limit must be between 1 and {maximum} entries"
				)
			}
			Self::TreeDepth { maximum } => {
				write!(formatter, "tree depth may be at most {maximum} directories")
			}
			Self::TreeLimit { maximum } => {
				write!(
					formatter,
					"tree limit must be between 1 and {maximum} entries"
				)
			}
			Self::UnsupportedVolume => formatter.write_str("unsupported volume identity"),
			Self::UnsupportedName => formatter.write_str("unsupported stored name encoding"),
			Self::StalledDirectory => {
				formatter.write_str("directory scan returned an empty incomplete page")
			}
			Self::ForkAbsent { kind } => write!(formatter, "the requested {kind:?} fork is absent"),
			Self::ForkLimit { length, maximum } => write!(
				formatter,
				"the requested fork contains {length} bytes and the output limit is {maximum} bytes"
			),
			Self::Allocation(_) => {
				formatter.write_str("could not allocate the volume query buffer")
			}
			Self::Output(_) => formatter.write_str("could not write the selected fork"),
			Self::View { action, .. } => write!(formatter, "could not {action}"),
		}
	}
}

impl<E: StdError + 'static> StdError for Error<E> {
	fn source(&self) -> Option<&(dyn StdError + 'static)> {
		match self {
			Self::InvalidPath(error) => Some(error),
			Self::Allocation(error) => Some(error),
			Self::Output(error) => Some(error),
			Self::View { source, .. } => Some(source),
			_ => None,
		}
	}
}

impl<E> From<TryReserveError> for Error<E> {
	fn from(error: TryReserveError) -> Self {
		Self::Allocation(error)
	}
}
