//! Shared contracts and infrastructure for hosted dmgez services.

#[cfg(feature = "staging")]
pub mod output;
#[cfg(all(
	feature = "native-process",
	any(target_os = "linux", target_os = "macos")
))]
pub mod process;
#[cfg(feature = "source")]
pub mod source;
