//! Generate and inspect Finder presentation stores for image folders.
//!
//! The writer supports icon and list layouts and integer icon positions.
//! Image backgrounds can use a file alias prepared from the final volume.
//! Each store uses one B-tree leaf. Writing it performs no filesystem access.

pub mod inspection;
mod layout;
mod records;
mod store;
#[cfg(feature = "volume-icon")]
pub mod volume_icon;

pub use dmg_layout::AliasRecord;
pub use store::{
	Error, LayoutItem, STORE_NAME, validate_options, validate_options_for_image_background,
	write_store, write_store_with_background_alias,
};
