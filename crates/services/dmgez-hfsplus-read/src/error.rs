use hfsplus_format::{Error as FormatError, hard_links};
use hfsplus_reader::{
	Error as VolumeError,
	allocation::AllocationError,
	attributes::AttributeReadError,
	catalog::{CatalogError, HardLinkError},
	check::CheckError as StructureError,
	overflow::OverflowError,
};
use thiserror::Error;

/// The stage and underlying cause of a failed HFS-family check.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CheckError<E> {
	#[error("could not open the HFS-family volume")]
	Open(#[source] VolumeError<E>),
	#[error("could not verify HFS-family structure")]
	Structure(#[source] StructureError<E>),
}

/// Category for verification reports that represent engine errors uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckFailureKind {
	Unsupported,
	Corrupt,
	ResourceLimit,
	Read,
}

use CheckFailureKind::{Corrupt, Read, ResourceLimit, Unsupported};

impl<E> CheckError<E> {
	/// Classifies a failure while retaining the underlying engine error.
	#[must_use]
	pub fn kind(&self) -> CheckFailureKind {
		match self {
			Self::Open(error) => volume_error(error),
			Self::Structure(error) => match error {
				StructureError::Allocation(error) => match error {
					AllocationError::Read(error) => volume_error(error),
					AllocationError::Fork { error, .. } => overflow_error(error),
					AllocationError::BitmapLimit { .. }
					| AllocationError::RangeLimit
					| AllocationError::Memory(_) => ResourceLimit,
					_ => Corrupt,
				},
				StructureError::Catalog(error) => catalog_error(error),
				StructureError::HardLink(error) => match error {
					HardLinkError::Catalog(error) => catalog_error(error),
					HardLinkError::Codec {
						error: hard_links::Error::Format(error),
						..
					} => format_error(*error),
					HardLinkError::DirectoryLink(_) => Unsupported,
					HardLinkError::ChainLimit(_) => ResourceLimit,
					_ => Corrupt,
				},
				StructureError::Attributes(error) => match error {
					AttributeReadError::Fork(error) => volume_error(error),
					AttributeReadError::Layout { error, .. } => overflow_error(error),
					AttributeReadError::Format { error, .. } => format_error(*error),
					AttributeReadError::UnsupportedTree => Unsupported,
					AttributeReadError::ValueLimit { .. }
					| AttributeReadError::NodeLimit
					| AttributeReadError::EntryLimit
					| AttributeReadError::Allocation(_) => ResourceLimit,
					_ => Corrupt,
				},
				StructureError::Fork { error, .. } => overflow_error(error),
				StructureError::Limit(_) | StructureError::Memory(_) => ResourceLimit,
				_ => Corrupt,
			},
		}
	}
}

fn volume_error<E>(error: &VolumeError<E>) -> CheckFailureKind {
	match error {
		VolumeError::Format(error) => format_error(*error),
		VolumeError::Source { .. } => Read,
		VolumeError::UnsupportedVolume(_) | VolumeError::OverflowExtents => Unsupported,
		_ => Corrupt,
	}
}

fn overflow_error<E>(error: &OverflowError<E>) -> CheckFailureKind {
	match error {
		OverflowError::Read(error) => volume_error(error),
		OverflowError::BootstrapOverflow => Unsupported,
		OverflowError::Limit(_) | OverflowError::Allocation(_) => ResourceLimit,
		_ => Corrupt,
	}
}

fn catalog_error<E>(error: &CatalogError<E>) -> CheckFailureKind {
	match error {
		CatalogError::Read(error) => volume_error(error),
		CatalogError::Format { error, .. } => format_error(*error),
		CatalogError::UnsupportedVolume(_)
		| CatalogError::UnsupportedComparison { .. }
		| CatalogError::UnsupportedTree => Unsupported,
		CatalogError::Limit(_) | CatalogError::Allocation(_) => ResourceLimit,
		_ => Corrupt,
	}
}

fn format_error(error: FormatError) -> CheckFailureKind {
	match error {
		FormatError::InvalidSignature
		| FormatError::UnsupportedVersion(_)
		| FormatError::UnsupportedCatalogRecord(_)
		| FormatError::UnsupportedAttributeRecord(_)
		| FormatError::UnsupportedAttributeSize(_)
		| FormatError::UnsupportedCompressedAttribute
		| FormatError::UnsupportedJournalFlags(_) => Unsupported,
		_ => Corrupt,
	}
}
