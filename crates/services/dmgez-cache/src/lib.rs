//! Descriptor-scoped storage for explicit compression cache requests.

mod disk;

pub use disk::{DirectoryIdentity, DiskCache, Error};
