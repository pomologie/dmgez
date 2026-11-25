//! Read-only volume selection, browsing and verification for bounded APFS extents.

mod container;
mod probe;
mod source;
mod verification;
mod volume;

pub use apfs_container::{
	Corruption as CheckpointCorruption, Diagnostic as CheckpointDiagnostic, VolumeSelector,
};
pub use apfs_format::{
	object::{ObjectId, TransactionId},
	volume::{VolumeName, VolumeRole},
};
pub use apfs_reader::{CheckData, CheckReport};
pub use container::{ApfsContainer, OpenError, VolumeInfo};
pub use probe::{ProbeError, probe, probe_partitionless};
pub use source::{SourceError, WindowSource};
pub use verification::{CheckError, CheckFailureKind, VerificationEvidence, check};
pub use volume::{AdapterError, ApfsBackend, ApfsDirectory, Node};
