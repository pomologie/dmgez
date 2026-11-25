//! Read-only volume selection and browsing for bounded APFS disk extents.

mod container;
mod probe;
mod source;
mod volume;

pub use apfs_container::{Corruption as CheckpointCorruption, Diagnostic as CheckpointDiagnostic};
pub use apfs_format::{
	object::{ObjectId, TransactionId},
	volume::{VolumeName, VolumeRole},
};
pub use container::{ApfsContainer, OpenError, VolumeInfo, VolumeSelector};
pub use probe::{ProbeError, probe_partitionless};
pub use source::{SourceError, WindowSource};
pub use volume::{AdapterError, ApfsBackend, ApfsDirectory, Node};
