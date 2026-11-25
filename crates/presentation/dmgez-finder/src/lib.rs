//! Generate and inspect Finder presentation stores for image folders.
//!
//! The writer supports icon, list, and column defaults and integer icon positions.
//! Image backgrounds can use a file alias prepared from the final volume.
//! Each store uses one B-tree leaf. Writing it performs no filesystem access.

mod background_alias;
#[cfg(feature = "volume-icon")]
mod finder_info;
pub mod inspection;
#[cfg(feature = "volume-icon")]
pub mod item_icon;
mod layout;
mod records;
mod store;
#[cfg(feature = "volume-icon")]
pub mod volume_icon;

pub use background_alias::{
	BackgroundAliasError, ImageBackgroundIdentity, build_image_background_alias,
	write_store_with_background_identity,
};
pub use dmg_layout::AliasRecord;
pub use store::{
	Error, LayoutItem, STORE_NAME, validate_options, validate_options_for_image_background,
	write_store, write_store_with_background_alias,
};
