//! Generate and inspect Finder presentation stores for image folders.
//!
//! The writer supports icon and list layouts and integer icon positions.
//! Each store uses one B-tree leaf and contains no host filesystem references.

pub mod inspection;
mod layout;
mod records;
mod store;
#[cfg(feature = "volume-icon")]
pub mod volume_icon;

pub use store::{Error, LayoutItem, STORE_NAME, validate_options, write_store};
