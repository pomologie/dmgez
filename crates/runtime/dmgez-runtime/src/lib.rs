//! Application-independent entry points for dmgez workflows.

#[cfg(feature = "image")]
pub mod disk;
#[cfg(feature = "image")]
pub mod image;
#[cfg(feature = "project-build")]
pub mod project_build;
#[cfg(feature = "project-input")]
pub mod project_input;
