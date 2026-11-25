//! Verified, copy-on-write rebuilds from project revisions.

use std::{
	collections::{BTreeMap, TryReserveError},
	io,
	path::{Path, PathBuf},
	thread,
};

use dmgez_builder::preflight::{Availability, ReleasePlan};
use dmgez_config::{
	MAX_MANIFEST_BYTES,
	project_edit::{EditBatchError, MAX_EDIT_BATCH_BYTES, ProjectEdit, parse_edit_batch},
};
use dmgez_operations::edit::{ProjectRebuildError, ProjectRebuildRequest, plan_project_rebuild};
use dmgez_udif::writer::Cancellation;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
	edit_input::{SHA256_HEX_BYTES, Snapshot, encode_sha256},
	project_build::{BuildEvent, BuildMode, metadata_loss_count},
	staged_image::verify_stage,
};

/// Inputs to an unsigned rebuild. Neither input document is modified.
#[derive(Debug)]
pub struct EditRequest<'source> {
	/// Existing project manifest. Relative paths use the caller's directory.
	pub project: &'source Path,
	/// Ordered TOML edit batch. Relative paths use the caller's directory.
	pub changes: &'source Path,
	/// New image destination in project syntax, relative to the project directory.
	pub output: String,
	/// Optional expected project revision as a case-insensitive SHA-256 hex string.
	pub expected_project_sha256: Option<&'source str>,
	/// Literal substitution values that take precedence over manifest definitions.
	pub definitions: BTreeMap<String, String>,
	/// Whether to seal a dry-run plan or stage and publish the verified image.
	pub mode: BuildMode,
}

/// Result of preparing or publishing a new unsigned image from the edit batch.
#[derive(Debug)]
#[non_exhaustive]
pub struct EditReport {
	/// Digest of the original bounded project text.
	pub project_sha256: String,
	/// Resolved destination of the new image.
	pub output: PathBuf,
	/// Number of caller-supplied edits, excluding the destination assignment.
	pub edits: usize,
	/// Number of resolved manifest entries.
	pub entries: usize,
	/// Exact logical length of the sealed image.
	pub logical_bytes: u64,
	/// Number of individual metadata fields that cannot be preserved.
	pub metadata_losses: usize,
	/// Whether configured release stages were deferred for the unsigned rebuild.
	pub release_stages_deferred: bool,
	/// Present once the rebuilt image has been published.
	pub publication: Option<Publication>,
}

/// Evidence from the publication of the new image.
#[derive(Debug)]
#[non_exhaustive]
pub struct Publication {
	/// Final encoded length in bytes.
	pub physical_bytes: u64,
	/// Whether staged output verification completed.
	pub verified: bool,
}

/// Errors reading, preparing, or publishing a project edit.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EditError {
	#[error("expected project SHA-256 must contain 64 hexadecimal digits")]
	InvalidExpectedDigest,
	#[error("project SHA-256 does not match the expected revision")]
	UnexpectedRevision,
	#[error("project path has no parent directory")]
	MissingProjectDirectory,
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Batch(#[from] EditBatchError),
	#[error(transparent)]
	TooManyEdits(#[from] TryReserveError),
	#[error(transparent)]
	Plan(#[from] ProjectRebuildError),
	#[error(transparent)]
	Build(#[from] dmgez_hfsplus::Error),
}

/// Seal an unsigned rebuild with resolved manifest edits and a new destination.
/// Recheck both input documents before staging and during staged UDIF verification.
///
/// # Errors
/// Returns an error for invalid or changed inputs, unsupported edits, resource
/// exhaustion, or a build, verification, or publication failure.
pub fn rebuild(
	request: EditRequest<'_>,
	cancellation: Cancellation,
	observe: impl Fn(BuildEvent<'_>),
) -> Result<EditReport, EditError> {
	let source = Snapshot::open(request.project, MAX_MANIFEST_BYTES)?;
	let project_sha256 = encode_sha256(&Sha256::digest(source.text.as_bytes()).into());
	check_expected(request.expected_project_sha256, &project_sha256)?;
	let changes = Snapshot::open(request.changes, MAX_EDIT_BATCH_BYTES)?;
	let mut edits = parse_edit_batch(&changes.text)?;
	let edit_count = edits.len();
	edits.try_reserve(1)?;
	edits.push(ProjectEdit::SetOutputPath(request.output));
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.verify = Availability::Available;
	let config_dir = source
		.path
		.parent()
		.ok_or(EditError::MissingProjectDirectory)?;
	let plan = plan_project_rebuild(ProjectRebuildRequest {
		expected_source: &source.text,
		current_source: &source.text,
		config_dir,
		defines: request.definitions,
		edits: &edits,
		capabilities,
		host_threads: thread::available_parallelism()?,
	})?;
	let mut project = plan.project().clone();
	// Edits require independent read-back before the builder may publish.
	project.build.verify_output = true;
	let progress = |event| observe(BuildEvent::Progress(event));
	let prepared = dmgez_hfsplus::prepare_with_progress(
		&project,
		plan.config_dir(),
		plan.options(),
		cancellation,
		progress,
	)?;
	let image = prepared.seal()?;
	let entries = prepared.prepared().manifest().entries().len();
	let logical_bytes = image.logical_bytes();
	let metadata_losses = metadata_loss_count(image.metadata_losses());
	observe(BuildEvent::MetadataLosses(image.metadata_losses()));
	source.validate()?;
	changes.validate()?;
	let publication = match request.mode {
		BuildMode::Plan => None,
		BuildMode::Publish => {
			let built = image.publish_with_progress(
				Some(&|stage, written| {
					verify_stage(stage, written)?;
					source.validate()?;
					changes.validate()
				}),
				progress,
			)?;
			Some(Publication {
				physical_bytes: built.report.image.physical_bytes,
				verified: built.report.verified,
			})
		}
	};
	Ok(EditReport {
		project_sha256,
		output: plan.output_path().to_owned(),
		edits: edit_count,
		entries,
		logical_bytes,
		metadata_losses,
		release_stages_deferred: prepared.prepared().profile().release()
			== ReleasePlan::ConfiguredStagesDeferred,
		publication,
	})
}

fn check_expected(expected: Option<&str>, actual: &str) -> Result<(), EditError> {
	let Some(expected) = expected else {
		return Ok(());
	};
	if expected.len() != SHA256_HEX_BYTES || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
	{
		return Err(EditError::InvalidExpectedDigest);
	}
	if !expected.eq_ignore_ascii_case(actual) {
		return Err(EditError::UnexpectedRevision);
	}
	Ok(())
}
