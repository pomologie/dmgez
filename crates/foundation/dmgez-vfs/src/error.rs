use dmgez_udif::Error as SourceError;
use thiserror::Error;

use crate::{PartitionId, Scheme, VolumeId};

/// Failure discovering, selecting, or streaming a logical disk extent.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Source(#[from] SourceError),
	#[error("corrupt {scheme} map: {reason}")]
	Corrupt {
		scheme: Scheme,
		reason: &'static str,
	},
	#[error("unsupported {scheme} map: {reason}")]
	Unsupported {
		scheme: Scheme,
		reason: &'static str,
	},
	#[error("partition discovery conflict: {reason}")]
	Conflict { reason: &'static str },
	#[error("resource limit: {0}")]
	Limit(&'static str),
	#[error("logical range {offset} + {len} exceeds {available} bytes")]
	OutOfRange {
		offset: u64,
		len: u64,
		available: u64,
	},
	#[error("volume selector is absent from this disk: {0:?}")]
	UnknownVolume(VolumeId),
	#[error("partition has no admitted filesystem data window: {0:?}")]
	NoDataWindow(PartitionId),
	#[error("no usable volume is available on this disk")]
	NoVolume,
	#[error("{candidates} usable volumes require an explicit selector")]
	AmbiguousVolumes { candidates: usize },
	#[error("a nonempty raw export requires a nonempty copy buffer")]
	EmptyCopyBuffer,
	#[error("raw export cancelled")]
	Cancelled,
	#[error("raw export sink failed")]
	Sink(#[source] std::io::Error),
	#[error("corrupt partitionless HFS volume geometry")]
	VolumeGeometry,
}
