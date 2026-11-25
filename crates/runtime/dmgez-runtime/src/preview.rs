//! Render project and disk-image folders for application frontends.

use std::path::Path;

use dmgez_config::{ConfigError, Project, VirtualPath};
use dmgez_core::output::{PublishError, publish_new};
pub use dmgez_preview::ItemVisibility;
use dmgez_preview::raster::{self, DesignPreview};
use thiserror::Error;

use crate::project_input::{self, InputError};

#[cfg(feature = "actual-preview")]
pub mod actual;

/// A design could not be loaded, rendered, or saved to a new file.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DesignError {
	#[error(transparent)]
	Input(#[from] InputError),
	#[error(transparent)]
	Config(#[from] ConfigError),
	#[error(transparent)]
	Raster(#[from] raster::Error),
	#[error(transparent)]
	Publish(#[from] PublishError<raster::Error>),
}

/// Publish one folder's declared design as a new PNG and return its pixel dimensions.
///
/// Project-relative assets are resolved against the manifest's directory.
///
/// # Errors
/// Returns an error for an invalid project, an unsupported design, or failed publication.
pub fn design_png(
	project: &Path,
	folder: &VirtualPath,
	visibility: ItemVisibility,
	output: &Path,
) -> Result<(u32, u32), DesignError> {
	let project_directory = project.parent().unwrap_or(Path::new(""));
	let project = Project::parse(&project_input::read(project)?)?;
	let preview = DesignPreview::from_project(&project, project_directory, folder, visibility)?;
	publish_new(output, |output| preview.write_png(output), |_, ()| Ok(()))?;
	Ok(preview.size())
}
