//! Application-independent entry points for dmgez workflows.

#[cfg(feature = "appdmg-import")]
pub mod appdmg;
#[cfg(feature = "compare")]
pub mod compare;
#[cfg(feature = "image")]
pub mod disk;
#[cfg(any(feature = "image-edit", feature = "project-edit"))]
mod edit_input;
#[cfg(feature = "encoding")]
pub mod encoding;
#[cfg(all(feature = "extraction", any(target_os = "linux", target_os = "macos")))]
pub mod extraction;
#[cfg(feature = "image")]
pub mod image;
#[cfg(feature = "image-edit")]
pub mod image_edit;
#[cfg(feature = "image")]
pub mod inspection;
#[cfg(all(
	feature = "installation",
	any(target_os = "linux", target_os = "macos")
))]
pub mod installation;
#[cfg(all(
	feature = "installer-test",
	any(target_os = "linux", target_os = "macos")
))]
pub mod installer_test;
#[cfg(feature = "lint")]
pub mod lint;
#[cfg(all(
	feature = "mount-watch",
	feature = "hfsplus-volume",
	feature = "verified-volume",
	target_os = "macos"
))]
pub mod mount_watch;
#[cfg(feature = "preview")]
pub mod preview;
#[cfg(feature = "project-build")]
pub mod project_build;
#[cfg(feature = "project-edit")]
pub mod project_edit;
#[cfg(feature = "project-input")]
pub mod project_input;
#[cfg(any(feature = "image-edit", feature = "project-edit"))]
mod staged_image;
#[cfg(any(feature = "hfsplus-volume", feature = "apfs-volume"))]
pub mod volume;
#[cfg(feature = "raw-wrap")]
pub mod wrap;
