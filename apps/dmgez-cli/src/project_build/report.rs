use std::{borrow::Cow, io::Write, path::Path};

use dmgez_builder::{
	execution::BuildReport,
	partition::Layout,
	preflight::{FilesystemProfile, FinderPlan, OutputPlan, ReleasePlan},
};
use dmgez_config::ImageFormat;
use dmgez_runtime::project_build::PlanSummary;
use serde::{Serialize, Serializer};

use crate::{CliResult, report_output::write_report};

use super::execute::write_dry_run;

const SCHEMA_VERSION: u16 = 1;

pub(super) fn plan(summary: &PlanSummary, json: bool) -> CliResult {
	let report = Report {
		schema_version: SCHEMA_VERSION,
		operation: Operation::Build,
		output: summary.output.as_deref().map(Path::to_string_lossy),
		format: format_name(summary.format),
		logical_bytes: Bytes(summary.logical_bytes),
		physical_bytes: summary.physical_bytes.map(Bytes),
		metadata_losses: summary.metadata_losses,
		release: release_name(summary.release),
		details: Details::DryRun {
			filesystem: filesystem_name(summary.filesystem),
			partition: partition_name(summary.partition),
			finder: finder_name(summary.finder),
			publication: summary.publication.into(),
			compression_cache: summary.compression_cache,
			resolved_entries: summary.resolved_entries,
			source_payload_bytes: Bytes(summary.source_payload_bytes),
			finder_assets: summary.finder_assets,
		},
	};
	write_report(json, &report, |output| write_dry_run(output, summary))
}

pub(super) fn published(
	build: &BuildReport,
	output: Option<&Path>,
	format: ImageFormat,
	losses: usize,
	json: bool,
) -> CliResult {
	let report = Report {
		schema_version: SCHEMA_VERSION,
		operation: Operation::Build,
		output: output.map(Path::to_string_lossy),
		format: format_name(format),
		logical_bytes: Bytes(build.image.logical_bytes),
		physical_bytes: Some(Bytes(build.image.physical_bytes)),
		metadata_losses: losses,
		release: release_name(build.release),
		details: Details::Published {
			verified: build.verified,
		},
	};
	write_report(json, &report, |output| {
		writeln!(
			output,
			"{} logical bytes, {} physical bytes, {losses} metadata losses",
			build.image.logical_bytes, build.image.physical_bytes,
		)
	})
}

#[derive(Serialize)]
struct Report<'path> {
	schema_version: u16,
	operation: Operation,
	output: Option<Cow<'path, str>>,
	format: &'static str,
	logical_bytes: Bytes,
	physical_bytes: Option<Bytes>,
	metadata_losses: usize,
	release: &'static str,
	#[serde(flatten)]
	details: Details,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
	Build,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum Details {
	DryRun {
		filesystem: &'static str,
		partition: &'static str,
		finder: &'static str,
		publication: Publication,
		compression_cache: bool,
		resolved_entries: usize,
		source_payload_bytes: Bytes,
		finder_assets: usize,
	},
	Published {
		verified: bool,
	},
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Publication {
	Path { overwrite: bool },
	Stream,
}

impl From<OutputPlan> for Publication {
	fn from(plan: OutputPlan) -> Self {
		match plan {
			OutputPlan::Path { overwrite } => Self::Path { overwrite },
			OutputPlan::Stream => Self::Stream,
		}
	}
}

struct Bytes(u64);

impl Serialize for Bytes {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(&self.0)
	}
}

const fn format_name(format: ImageFormat) -> &'static str {
	match format {
		ImageFormat::Udro => "udro",
		ImageFormat::Udzo => "udzo",
		ImageFormat::Ulfo => "ulfo",
		ImageFormat::Ulmo => "ulmo",
		ImageFormat::Raw => "raw",
		ImageFormat::Udrw => "udrw",
		ImageFormat::Udsb => "udsb",
		ImageFormat::Sparseimage => "sparseimage",
		ImageFormat::Asif => "asif",
		ImageFormat::Udbz => "udbz",
		ImageFormat::Udco => "udco",
		_ => "unknown",
	}
}

const fn filesystem_name(filesystem: FilesystemProfile) -> &'static str {
	match filesystem {
		FilesystemProfile::HfsplusAscii => "hfsplus_ascii",
		FilesystemProfile::HfsplusUnicode => "hfsplus_unicode",
		_ => "unknown",
	}
}

const fn partition_name(layout: Layout) -> &'static str {
	match layout {
		Layout::Partitionless => "none",
		Layout::Apm => "apm",
		Layout::Gpt => "gpt",
		Layout::Mbr => "mbr",
		_ => "unknown",
	}
}

const fn finder_name(finder: FinderPlan) -> &'static str {
	match finder {
		FinderPlan::NoStore => "no_store",
		FinderPlan::IconView => "icon_view",
		FinderPlan::ListView => "list_view",
		FinderPlan::ColumnView => "column_view",
	}
}

const fn release_name(release: ReleasePlan) -> &'static str {
	match release {
		ReleasePlan::Unsigned => "unsigned",
		ReleasePlan::ConfiguredStagesDeferred => "configured_stages_deferred",
		ReleasePlan::Requested => "requested",
	}
}
