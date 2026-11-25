//! Project-file admission for layout and installer diagnostics.

use std::path::Path;

use dmgez_config::{ConfigError, Project, VirtualPath};
use dmgez_lint::{Limits, Report, Suppression, lint_layout};
use dmgez_preview::{DeclaredTree, DeclaredTreeError};
use thiserror::Error;

use crate::project_input::{self, InputError};

#[cfg(all(feature = "installer-lint", any(unix, windows)))]
pub mod installer;

/// A project could not be admitted or linted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LayoutError {
	#[error(transparent)]
	Input(#[from] InputError),
	#[error(transparent)]
	Config(#[from] ConfigError),
	#[error(transparent)]
	Tree(#[from] DeclaredTreeError),
	#[error("no declared project folder at {:?}", .0.as_str())]
	MissingFolder(VirtualPath),
	#[error(transparent)]
	Lint(#[from] dmgez_lint::Error),
}

/// An admitted project and its declaration tree for repeated layout checks.
#[derive(Debug)]
pub struct LayoutProject {
	project: Project,
	tree: DeclaredTree,
}

impl LayoutProject {
	/// Loads and indexes a bounded project manifest without following payload sources.
	///
	/// # Errors
	/// Returns an error for unreadable or invalid projects or over-limit trees.
	pub fn load(path: &Path) -> Result<Self, LayoutError> {
		let project = Project::parse(&project_input::read(path)?)?;
		let tree = DeclaredTree::from_project(&project)?;
		Ok(Self { project, tree })
	}

	/// Lints a declared folder's layout within the supplied diagnostic limits.
	///
	/// # Errors
	/// Returns an error for a missing folder, an invalid layout, exhausted limits,
	/// or cancellation.
	pub fn lint<'input>(
		&'input self,
		folder: &VirtualPath,
		limits: Limits,
		suppressions: &[Suppression<'input>],
		cancelled: impl FnMut() -> bool,
	) -> Result<Report<'input>, LayoutError> {
		if !self.tree.contains_folder(folder) {
			return Err(LayoutError::MissingFolder(folder.clone()));
		}
		let layout = self.tree.layout(&self.project, folder)?;
		Ok(lint_layout(&layout, limits, suppressions, cancelled)?)
	}
}
