//! Shared contracts and infrastructure for hosted dmgez services.

#![cfg_attr(
	not(all(feature = "native-directory-privacy", target_os = "macos")),
	forbid(unsafe_code)
)]

#[cfg(feature = "staging")]
pub mod output;
#[cfg(all(feature = "native-directory-privacy", target_os = "macos"))]
pub mod private_directory;
#[cfg(all(
	feature = "native-process",
	any(target_os = "linux", target_os = "macos")
))]
pub mod process;
#[cfg(feature = "source")]
pub mod source;
