//! Discover partition geometry in an immutable logical byte source (D-READ-01).
//!
//! Container block maps translate logical reads. They do not supply partition
//! identity. Filesystem interpretation is performed by the selected engine.

mod apm;
mod discovery;
mod disk;
mod error;
mod gpt;
mod layout;
mod mbr;
mod partition;
pub mod volume;
mod window;

#[cfg(kani)]
#[path = "../verification/kani/window.rs"]
mod window_proof;

pub use disk::{Disk, LogicalDisk, Raw, VolumeId};
pub use error::Error;
pub use layout::{Filesystem, Layout, Limits};
pub use partition::{Partition, PartitionId, PartitionKind, PartitionRole, Scheme};
pub use window::Window;
