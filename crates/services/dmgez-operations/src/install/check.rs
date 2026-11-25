//! Read-only checks of a finished installer's filesystem view.

use std::{collections::TryReserveError, error::Error as StdError, hash::Hash};

use dmgez_vfs::volume::{NodeKind, ViewError, VolumeBackend, VolumeView};
use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::extract::{NameLimits, NamePlan, NamePlanError, NameProfile, preflight_names};

use super::{Error as ApplicationError, RequiredMember, is_application_name, validate_layout};

const ROOT: &str = "/";
const APPLICATIONS_NAME: &str = "Applications";
const APPLICATIONS_PATH: &str = "/Applications";
const PATH_SEPARATOR: char = '/';
const LINK_SEPARATOR: u8 = b'/';
const CURRENT_COMPONENT: &[u8] = b".";
const PARENT_COMPONENT: &[u8] = b"..";
const NUL: u8 = b'\0';
const DEFAULT_MAX_APPLICATIONS: usize = 128;
const DEFAULT_MAX_DIAGNOSTICS: usize = 512;
const DEFAULT_MAX_LINK_BYTES: usize = 4_096;
const IMAGE_NAME_PROFILE: NameProfile = NameProfile::PosixUtf8;

/// Bounds for an image-side installer check.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct InstallerLimits {
	pub names: NameLimits,
	/// Bounds the number of application property lists opened.
	pub max_applications: usize,
	pub max_diagnostics: usize,
	/// Largest link payload admitted by this profile after the reader opens it.
	pub max_link_bytes: usize,
}

impl Default for InstallerLimits {
	fn default() -> Self {
		Self {
			names: NameLimits::default(),
			max_applications: DEFAULT_MAX_APPLICATIONS,
			max_diagnostics: DEFAULT_MAX_DIAGNOSTICS,
			max_link_bytes: DEFAULT_MAX_LINK_BYTES,
		}
	}
}

/// Checked filesystem rules for one image volume.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct InstallerReport {
	/// Number of top-level directories with an application bundle name.
	pub applications: usize,
	pub diagnostics: Vec<InstallerDiagnostic>,
}

impl InstallerReport {
	#[must_use]
	pub fn is_valid(&self) -> bool {
		self.diagnostics.is_empty()
	}
}

/// A failed image-side installer rule, with exact on-volume evidence.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct InstallerDiagnostic {
	pub rule: InstallerRule,
	pub path: String,
	/// Literal link bytes when the finding involves a symbolic link payload.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub target: Option<Vec<u8>>,
}

/// Stable image-side installer rule identifiers.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum InstallerRule {
	MissingApplication,
	InvalidApplicationEntry,
	MissingApplicationsLink,
	InvalidApplicationsLink,
	InvalidSymlinkTarget,
	ExternalSymlink,
	UnverifiedSymlinkTarget,
	BundleContentsMissing,
	BundleInfoMissing,
	BundleInfoInvalid,
	BundleExecutableDirectoryMissing,
	BundleExecutableMissing,
	BundleExecutableInvalid,
}

impl InstallerRule {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::MissingApplication => "missing_application",
			Self::InvalidApplicationEntry => "invalid_application_entry",
			Self::MissingApplicationsLink => "missing_applications_link",
			Self::InvalidApplicationsLink => "invalid_applications_link",
			Self::InvalidSymlinkTarget => "invalid_symlink_target",
			Self::ExternalSymlink => "external_symlink",
			Self::UnverifiedSymlinkTarget => "unverified_symlink_target",
			Self::BundleContentsMissing => "bundle_contents_missing",
			Self::BundleInfoMissing => "bundle_info_missing",
			Self::BundleInfoInvalid => "bundle_info_invalid",
			Self::BundleExecutableDirectoryMissing => "bundle_executable_directory_missing",
			Self::BundleExecutableMissing => "bundle_executable_missing",
			Self::BundleExecutableInvalid => "bundle_executable_invalid",
		}
	}
}

impl Serialize for InstallerRule {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(self.as_str())
	}
}

/// A failure that prevented a complete read-only installer check.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InstallerError<E: StdError + 'static> {
	#[error("could not open the installer volume root")]
	Root(#[source] ViewError<E>),
	#[error(transparent)]
	Names(#[from] NamePlanError<E>),
	#[error("could not inspect installer application {path:?}")]
	Application {
		path: String,
		#[source]
		source: ApplicationError<E>,
	},
	#[error("could not inspect installer symbolic link {path:?}")]
	Symlink {
		path: String,
		#[source]
		source: ViewError<E>,
	},
	#[error("installer symbolic link {path:?} exceeds the payload limit of {limit} bytes")]
	LinkLimit { path: String, limit: usize },
	#[error("installer check exceeds its application limit")]
	ApplicationLimit,
	#[error("installer check exceeds its diagnostic limit")]
	DiagnosticLimit,
	#[error("could not reserve installer check memory")]
	Reserve(#[from] TryReserveError),
	#[error("installer check was cancelled")]
	Cancelled,
}

/// Checks applications, the Applications shortcut, and symbolic links in one
/// selected volume. Every path is inspected in place without following links.
/// The parser reads only the bounded app metadata and literal link payloads.
/// A subsequent parent component after a descent is unverified because the
/// descent could pass through a chain of symbolic links.
///
/// # Errors
/// Returns an error for unreadable filesystem metadata, unsupported names,
/// exhausted limits, or cancellation. Structural findings are in the report.
pub fn check_installer<B: VolumeBackend>(
	view: &mut VolumeView<B>,
	limits: InstallerLimits,
	mut cancelled: impl FnMut() -> bool,
) -> Result<InstallerReport, InstallerError<B::Error>>
where
	B::Id: Eq + Hash,
{
	if cancelled() {
		return Err(InstallerError::Cancelled);
	}
	let root = view.root().map_err(InstallerError::Root)?;
	let names = preflight_names(
		view,
		&root,
		IMAGE_NAME_PROFILE,
		limits.names,
		&mut cancelled,
	)
	.map_err(|error| match error {
		NamePlanError::Cancelled => InstallerError::Cancelled,
		error => InstallerError::Names(error),
	})?;
	let mut report = InstallerReport {
		applications: 0,
		diagnostics: Vec::new(),
	};
	let mut depths = Vec::new();
	depths.try_reserve_exact(names.entries().len())?;
	let mut shortcut = false;
	for (index, entry) in names.entries().iter().enumerate() {
		if cancelled() {
			return Err(InstallerError::Cancelled);
		}
		let parent_depth = entry.parent.map_or(0, |parent| depths[parent.index()]);
		depths.push(parent_depth + 1);
		let top_level = entry.parent.is_none();
		let name = entry.name.as_str();
		if top_level && is_application_name(name) {
			let path = destination(&names, index)?;
			if entry.kind == NodeKind::Directory {
				if report.applications >= limits.max_applications {
					return Err(InstallerError::ApplicationLimit);
				}
				report.applications += 1;
				let plan = preflight_names(
					view,
					&entry.source,
					IMAGE_NAME_PROFILE,
					limits.names,
					&mut cancelled,
				)
				.map_err(ApplicationError::Names);
				let checked = plan.and_then(|plan| {
					validate_layout(
						view,
						&plan,
						limits.names.max_component_bytes,
						&mut cancelled,
					)
				});
				if let Err(error) = checked {
					let rule = application_rule(error, &path)?;
					emit(&mut report, limits.max_diagnostics, rule, path, None)?;
				}
			} else {
				emit(
					&mut report,
					limits.max_diagnostics,
					InstallerRule::InvalidApplicationEntry,
					path,
					None,
				)?;
			}
		}
		let is_shortcut = top_level && name == APPLICATIONS_NAME;
		shortcut |= is_shortcut;
		if entry.kind == NodeKind::Symlink {
			let path = destination(&names, index)?;
			let target = match view.read_link(&entry.source) {
				Ok(target) => Some(target),
				Err(ViewError::MissingLink) => None,
				Err(source) => return Err(InstallerError::Symlink { path, source }),
			};
			if target
				.as_ref()
				.is_some_and(|target| target.len() > limits.max_link_bytes)
			{
				return Err(InstallerError::LinkLimit {
					path,
					limit: limits.max_link_bytes,
				});
			}
			let rule = if is_shortcut {
				(target.as_deref() != Some(APPLICATIONS_PATH.as_bytes()))
					.then_some(InstallerRule::InvalidApplicationsLink)
			} else {
				target
					.as_deref()
					.map_or(Some(InstallerRule::InvalidSymlinkTarget), |target| {
						link_rule(target, parent_depth)
					})
			};
			if let Some(rule) = rule {
				emit(&mut report, limits.max_diagnostics, rule, path, target)?;
			}
		} else if is_shortcut {
			emit(
				&mut report,
				limits.max_diagnostics,
				InstallerRule::InvalidApplicationsLink,
				APPLICATIONS_PATH.to_owned(),
				None,
			)?;
		}
	}
	if report.applications == 0 {
		emit(
			&mut report,
			limits.max_diagnostics,
			InstallerRule::MissingApplication,
			ROOT.to_owned(),
			None,
		)?;
	} else if !shortcut {
		emit(
			&mut report,
			limits.max_diagnostics,
			InstallerRule::MissingApplicationsLink,
			APPLICATIONS_PATH.to_owned(),
			None,
		)?;
	}
	report.diagnostics.sort_unstable_by(|a, b| {
		a.path
			.cmp(&b.path)
			.then_with(|| a.rule.as_str().cmp(b.rule.as_str()))
	});
	Ok(report)
}

fn application_rule<E: StdError + 'static>(
	error: ApplicationError<E>,
	path: &str,
) -> Result<InstallerRule, InstallerError<E>> {
	use InstallerRule as Rule;
	Ok(match error {
		ApplicationError::BundleMember(RequiredMember::Contents) => Rule::BundleContentsMissing,
		ApplicationError::BundleMember(RequiredMember::InfoPlist)
		| ApplicationError::MissingInfo => Rule::BundleInfoMissing,
		ApplicationError::Info => Rule::BundleInfoInvalid,
		ApplicationError::BundleMember(RequiredMember::ExecutableDirectory) => {
			Rule::BundleExecutableDirectoryMissing
		}
		ApplicationError::BundleMember(RequiredMember::MainExecutable) => {
			Rule::BundleExecutableMissing
		}
		ApplicationError::Executable => Rule::BundleExecutableInvalid,
		ApplicationError::Cancelled | ApplicationError::Names(NamePlanError::Cancelled) => {
			return Err(InstallerError::Cancelled);
		}
		source => {
			return Err(InstallerError::Application {
				path: path.to_owned(),
				source,
			});
		}
	})
}

fn link_rule(target: &[u8], mut depth: usize) -> Option<InstallerRule> {
	if target.is_empty() || target.contains(&NUL) {
		return Some(InstallerRule::InvalidSymlinkTarget);
	}
	if target.first() == Some(&LINK_SEPARATOR) {
		return Some(InstallerRule::ExternalSymlink);
	}
	let mut descended = false;
	let mut unverified = false;
	for component in target.split(|&byte| byte == LINK_SEPARATOR) {
		match component {
			[] | CURRENT_COMPONENT => (),
			PARENT_COMPONENT => {
				if depth == 0 {
					return Some(InstallerRule::ExternalSymlink);
				}
				unverified |= descended;
				depth -= 1;
			}
			_ => {
				descended = true;
				depth += 1;
			}
		}
	}
	unverified.then_some(InstallerRule::UnverifiedSymlinkTarget)
}

fn destination<Id>(names: &NamePlan<Id>, index: usize) -> Result<String, TryReserveError> {
	let mut components = Vec::new();
	let mut current = Some(index);
	while let Some(index) = current {
		let entry = &names.entries()[index];
		components.try_reserve(1)?;
		components.push(entry.name.as_str());
		current = entry.parent.map(crate::extract::EntryId::index);
	}
	let mut path = String::new();
	for name in components.into_iter().rev() {
		path.try_reserve(PATH_SEPARATOR.len_utf8() + name.len())?;
		path.push(PATH_SEPARATOR);
		path.push_str(name);
	}
	Ok(path)
}

fn emit<E: StdError + 'static>(
	report: &mut InstallerReport,
	limit: usize,
	rule: InstallerRule,
	path: String,
	target: Option<Vec<u8>>,
) -> Result<(), InstallerError<E>> {
	if report.diagnostics.len() >= limit {
		return Err(InstallerError::DiagnosticLimit);
	}
	report.diagnostics.try_reserve(1)?;
	report
		.diagnostics
		.push(InstallerDiagnostic { rule, path, target });
	Ok(())
}
