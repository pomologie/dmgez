//! APFS adapter used by the independent dmgez C5 compatibility witness.

mod volume;

pub use volume::{AdapterError, ApfsBackend, ApfsDirectory, Node, SourceError, WindowSource};
