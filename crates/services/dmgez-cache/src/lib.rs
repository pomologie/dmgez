//! Descriptor-scoped storage for explicit compression cache requests.

#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

mod disk;

pub use disk::{DirectoryIdentity, DiskCache, Error};
