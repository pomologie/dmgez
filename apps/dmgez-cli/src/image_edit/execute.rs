use std::{borrow::Cow, io::Write};

use dmgez_config::MetadataPolicy;
use dmgez_runtime::image_edit::{
	EditMode, EditRequest, SourceEnvelope, edit, encode_sha256, parse_expected_revision,
};
use serde::Serialize;

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
};

use super::{Arguments, HostMetadataPolicy};

const SCHEMA_VERSION: u16 = 1;
const UNSIGNED: &str = "unsigned";
const SIGNED_DIGESTS_VERIFIED: &str = "signed_digests_verified";
const MAX_LOSS_DETAILS: usize = 20;

pub(super) fn run(arguments: &Arguments) -> CliResult {
	let result = edit(EditRequest {
		image: &arguments.image,
		changes: &arguments.changes,
		output: &arguments.output,
		expected_revision: arguments
			.expect_image_sha256
			.as_deref()
			.map(parse_expected_revision)
			.transpose()?,
		metadata_policy: match arguments.metadata_policy {
			HostMetadataPolicy::Strict => MetadataPolicy::Strict,
			HostMetadataPolicy::Report => MetadataPolicy::Report,
		},
		mode: if arguments.dry_run {
			EditMode::Plan
		} else {
			EditMode::Publish
		},
	})?;
	let publication = result.publication.as_ref();
	let report = Report {
		schema_version: SCHEMA_VERSION,
		image_sha256: encode_sha256(result.revision.sha256()),
		source_envelope: match result.source_envelope {
			SourceEnvelope::Unsigned => UNSIGNED,
			SourceEnvelope::SignedDigestsVerified => SIGNED_DIGESTS_VERIFIED,
		},
		output: arguments.output.to_string_lossy(),
		edits: result.edits,
		entries: result.entries,
		metadata_losses: result
			.metadata_losses
			.iter()
			.map(|entry| entry.details.len())
			.sum(),
		metadata_loss_details: result
			.metadata_losses
			.iter()
			.flat_map(|entry| entry.details.iter().map(move |loss| (entry, loss)))
			.take(MAX_LOSS_DETAILS)
			.map(|(entry, loss)| MetadataLossDetail {
				path: entry.path.as_str(),
				other_aliases: entry.aliases.len(),
				field: format!("{:?}", loss.field),
				reason: format!("{:?}", loss.reason),
			})
			.collect(),
		logical_bytes: result.logical_bytes,
		physical_bytes: publication.map(|value| value.physical_bytes),
		logical_sha256: publication.map(|value| encode_sha256(&value.logical_sha256)),
		published: publication.is_some(),
		verified: publication.is_some(),
		trust: UNSIGNED,
	};
	report.write(arguments.json)
}

#[derive(Serialize)]
struct MetadataLossDetail<'path> {
	path: &'path str,
	other_aliases: usize,
	field: String,
	reason: String,
}

#[derive(Serialize)]
struct Report<'path> {
	schema_version: u16,
	image_sha256: String,
	source_envelope: &'static str,
	output: Cow<'path, str>,
	edits: usize,
	entries: usize,
	metadata_losses: usize,
	metadata_loss_details: Vec<MetadataLossDetail<'path>>,
	logical_bytes: u64,
	physical_bytes: Option<u64>,
	logical_sha256: Option<String>,
	published: bool,
	verified: bool,
	trust: &'static str,
}

impl Report<'_> {
	fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |output| {
			writeln!(output, "image_sha256 = {}", self.image_sha256)?;
			writeln!(output, "source_envelope = {}", self.source_envelope)?;
			writeln!(output, "output = {}", EscapedText(&self.output))?;
			writeln!(output, "edits = {}", self.edits)?;
			writeln!(output, "entries = {}", self.entries)?;
			writeln!(output, "metadata_losses = {}", self.metadata_losses)?;
			for loss in &self.metadata_loss_details {
				writeln!(
					output,
					"Metadata loss at {} ({} other aliases), {} ({})",
					EscapedText(loss.path),
					loss.other_aliases,
					loss.field,
					loss.reason,
				)?;
			}
			let hidden = self.metadata_losses - self.metadata_loss_details.len();
			if hidden != 0 {
				writeln!(output, "{hidden} additional metadata losses")?;
			}
			writeln!(output, "logical_bytes = {}", self.logical_bytes)?;
			if let Some(bytes) = self.physical_bytes {
				writeln!(output, "physical_bytes = {bytes}")?;
			}
			if let Some(digest) = &self.logical_sha256 {
				writeln!(output, "logical_sha256 = {digest}")?;
			}
			writeln!(output, "published = {}", self.published)?;
			writeln!(output, "verified = {}", self.verified)?;
			writeln!(output, "trust = {}", self.trust)
		})
	}
}
