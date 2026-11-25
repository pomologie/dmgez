use std::{
	borrow::Cow,
	fmt::Write as _,
	io,
	io::Write,
	path::{Path, PathBuf},
};

use dmgez_builder::preflight::{Availability, ReleasePlan};
use dmgez_config::{
	MAX_MANIFEST_BYTES,
	project_edit::{MAX_EDIT_BATCH_BYTES, ProjectEdit, parse_edit_batch},
};
use dmgez_operations::edit::{ProjectRebuildRequest, plan_project_rebuild};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
	CliResult,
	project_build::{Progress, report_losses},
	project_input,
	report_output::{EscapedText, write_report},
	staged_image::verify_stage,
};

use super::Arguments;

const SCHEMA_VERSION: u16 = 1;
const SHA256_HEX_BYTES: usize = 64;
const EXPECTED_DIGEST_FORMAT: &str = "expected project SHA-256 must contain 64 hexadecimal digits";
const EXPECTED_DIGEST_MISMATCH: &str = "project SHA-256 does not match the expected revision";
const UNSIGNED_TRUST: &str = "unsigned";

pub(super) fn run(arguments: &Arguments) -> CliResult {
	let source = Snapshot::open(&arguments.project, MAX_MANIFEST_BYTES)?;
	let project_sha256 = digest(&source.text);
	check_expected(arguments.expect_project_sha256.as_deref(), &project_sha256)?;
	let changes = Snapshot::open(&arguments.changes, MAX_EDIT_BATCH_BYTES)?;
	let mut edits = parse_edit_batch(&changes.text)?;
	let edit_count = edits.len();
	edits.try_reserve(1)?;
	edits.push(ProjectEdit::SetOutputPath(arguments.output.clone()));
	let mut capabilities = dmgez_hfsplus::capabilities();
	capabilities.verify = Availability::Available;
	let config_dir = source
		.path
		.parent()
		.ok_or("project path has no parent directory")?;
	let plan = plan_project_rebuild(ProjectRebuildRequest {
		expected_source: &source.text,
		current_source: &source.text,
		config_dir,
		defines: project_input::definitions(&arguments.define)?,
		edits: &edits,
		capabilities,
		host_threads: std::thread::available_parallelism()?,
	})?;
	let mut project = plan.project().clone();
	// An edit always checks the new container and the input revisions before
	// the ordinary builder may publish the no-clobber stage.
	project.build.verify_output = true;
	let progress = Progress::default();
	let prepared = dmgez_hfsplus::prepare_with_progress(
		&project,
		plan.config_dir(),
		plan.options(),
		Default::default(),
		|event| progress.update(event),
	)?;
	let image = prepared.seal()?;
	let mut report = Report {
		schema_version: SCHEMA_VERSION,
		project_sha256,
		output: plan.output_path().to_string_lossy(),
		edits: edit_count,
		entries: prepared.prepared().manifest().entries().len(),
		logical_bytes: image.logical_bytes(),
		physical_bytes: None,
		metadata_losses: report_losses(image.metadata_losses()),
		published: false,
		verified: false,
		trust: UNSIGNED_TRUST,
		release_stages_deferred: prepared.prepared().profile().release()
			== ReleasePlan::ConfiguredStagesDeferred,
	};
	source.validate()?;
	changes.validate()?;
	if !arguments.dry_run {
		let built = image.publish_with_progress(
			Some(&|stage, written| {
				verify_stage(stage, written)?;
				source.validate()?;
				changes.validate()
			}),
			|event| progress.update(event),
		)?;
		report.published = true;
		report.verified = built.report.verified;
		report.physical_bytes = Some(built.report.image.physical_bytes);
	}
	report.write(arguments.json)
}

struct Snapshot {
	path: PathBuf,
	text: String,
	limit: usize,
}

impl Snapshot {
	fn open(path: &Path, limit: usize) -> CliResult<Self> {
		let path = path.canonicalize()?;
		let text = project_input::read_limited(&path, limit)?;
		Ok(Self { path, text, limit })
	}

	fn validate(&self) -> io::Result<()> {
		let observed = project_input::read_limited(&self.path, self.limit).map_err(|error| {
			io::Error::other(format!(
				"could not recheck edit input {:?}: {error}",
				self.path
			))
		})?;
		if observed != self.text {
			return Err(io::Error::other(format!(
				"edit input changed after it was read: {:?}",
				self.path
			)));
		}
		Ok(())
	}
}

fn digest(project: &str) -> String {
	let mut encoded = String::with_capacity(SHA256_HEX_BYTES);
	for byte in Sha256::digest(project.as_bytes()) {
		write!(encoded, "{byte:02x}").expect("writing to a string is infallible");
	}
	encoded
}

fn check_expected(expected: Option<&str>, actual: &str) -> CliResult {
	let Some(expected) = expected else {
		return Ok(());
	};
	if expected.len() != SHA256_HEX_BYTES || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
	{
		return Err(EXPECTED_DIGEST_FORMAT.into());
	}
	if !expected.eq_ignore_ascii_case(actual) {
		return Err(EXPECTED_DIGEST_MISMATCH.into());
	}
	Ok(())
}

#[derive(Serialize)]
struct Report<'path> {
	schema_version: u16,
	project_sha256: String,
	output: Cow<'path, str>,
	edits: usize,
	entries: usize,
	logical_bytes: u64,
	physical_bytes: Option<u64>,
	metadata_losses: usize,
	published: bool,
	verified: bool,
	trust: &'static str,
	release_stages_deferred: bool,
}

impl Report<'_> {
	fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |output| {
			writeln!(output, "project_sha256 = {}", self.project_sha256)?;
			writeln!(output, "output = {}", EscapedText(&self.output))?;
			writeln!(output, "edits = {}", self.edits)?;
			writeln!(output, "entries = {}", self.entries)?;
			writeln!(output, "logical_bytes = {}", self.logical_bytes)?;
			if let Some(bytes) = self.physical_bytes {
				writeln!(output, "physical_bytes = {bytes}")?;
			}
			writeln!(output, "metadata_losses = {}", self.metadata_losses)?;
			writeln!(output, "published = {}", self.published)?;
			writeln!(output, "verified = {}", self.verified)?;
			writeln!(output, "trust = {}", self.trust)?;
			writeln!(
				output,
				"release_stages_deferred = {}",
				self.release_stages_deferred
			)
		})
	}
}
