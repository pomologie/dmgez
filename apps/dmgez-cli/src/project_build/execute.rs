use std::{
	cell::Cell,
	io::{self, Write},
	path::Path,
};

use dmgez_builder::{partition::Layout, preflight::ReleasePlan, progress::BuildProgress};
use dmgez_config::ConfigOverride;
use dmgez_hfsplus::MetadataLoss;
use dmgez_runtime::project_build::{
	BuildError, BuildEvent, BuildMode, BuildOutcome, BuildProject, PlanSummary, ProjectRequest,
};

use crate::{CliResult, project_input, report_output::EscapedText};

#[cfg(feature = "project-release")]
use super::release;
use super::{Arguments, report};

const MAX_LOSS_DETAILS: usize = 20;
const READ_PROGRESS_STEPS: u64 = 10;
const DUPLICATE_OVERWRITE: &str = "choose either --overwrite or --set build.overwrite";
#[cfg(not(feature = "project-release"))]
const RELEASE_FEATURE_REQUIRED: &str = "release builds require the project-release feature";
const RELEASE_DEFERRED: &str =
	"Configured release stages were deferred. Published an unsigned image.";

pub(crate) fn build(arguments: Arguments, working_directory: &Path) -> CliResult {
	#[cfg(not(feature = "project-release"))]
	if arguments.release {
		return Err(RELEASE_FEATURE_REQUIRED.into());
	}
	let definitions = project_input::definitions(&arguments.define)?;
	let overrides = arguments
		.set
		.iter()
		.map(|assignment| ConfigOverride::parse(assignment))
		.collect::<Result<Vec<_>, _>>()?;
	let project = match BuildProject::load(ProjectRequest {
		path: &arguments.project,
		working_directory,
		definitions: &definitions,
		overrides,
		overwrite: arguments.overwrite,
	}) {
		Err(BuildError::DuplicateOverwrite) => return Err(DUPLICATE_OVERWRITE.into()),
		result => result?,
	};
	let progress = Progress::default();
	let mode = if arguments.dry_run {
		BuildMode::Plan
	} else {
		BuildMode::Publish
	};
	let observe = |event: BuildEvent<'_>| match event {
		BuildEvent::Progress(event) => progress.update(event),
		BuildEvent::MetadataLosses(entries) => {
			report_losses(entries);
		}
	};
	#[cfg(feature = "project-release")]
	if arguments.release {
		return release::run(&project, &arguments, working_directory, mode, observe);
	}
	let (build, output, format, losses) = match project.run(mode, Default::default(), observe)? {
		BuildOutcome::Planned(summary) => {
			return report::plan(&summary, arguments.json);
		}
		BuildOutcome::Published {
			report,
			output,
			format,
			metadata_losses,
		} => (report, output, format, metadata_losses),
	};
	if build.release == ReleasePlan::ConfiguredStagesDeferred {
		writeln!(io::stderr().lock(), "{RELEASE_DEFERRED}")?;
	}
	#[cfg(feature = "compression-cache")]
	if let Some(cache) = build.cache {
		let mut output = io::stderr().lock();
		writeln!(output, "cache_hits={}", cache.hits)?;
		writeln!(output, "cache_misses={}", cache.misses)?;
		writeln!(output, "cache_rejected={}", cache.rejected)?;
	}
	report::published(&build, output.as_deref(), format, losses, arguments.json)
}

pub(super) fn write_dry_run(mut output: impl Write, summary: &PlanSummary) -> io::Result<()> {
	writeln!(output, "mode=dry-run")?;
	writeln!(output, "format={:?}", summary.format)?;
	writeln!(output, "filesystem={:?}", summary.filesystem)?;
	if summary.partition != Layout::Partitionless {
		writeln!(output, "partition={:?}", summary.partition)?;
	}
	writeln!(output, "finder={:?}", summary.finder)?;
	writeln!(output, "output={:?}", summary.output)?;
	writeln!(output, "publication={:?}", summary.publication)?;
	writeln!(output, "release={:?}", summary.release)?;
	if summary.compression_cache {
		writeln!(output, "compression_cache=enabled")?;
	}
	writeln!(output, "logical_bytes={}", summary.logical_bytes)?;
	if let Some(physical_bytes) = summary.physical_bytes {
		writeln!(output, "physical_bytes={physical_bytes}")?;
	} else {
		writeln!(output, "physical_bytes=unknown")?;
	}
	writeln!(output, "resolved_entries={}", summary.resolved_entries)?;
	writeln!(
		output,
		"source_payload_bytes={}",
		summary.source_payload_bytes
	)?;
	writeln!(output, "finder_assets={}", summary.finder_assets)?;
	writeln!(output, "metadata_losses={}", summary.metadata_losses)?;
	Ok(())
}

pub(crate) fn report_losses(entries: &[MetadataLoss<'_>]) -> usize {
	let total = entries.iter().map(|entry| entry.details.len()).sum();
	let details = entries
		.iter()
		.flat_map(|entry| entry.details.iter().map(move |loss| (entry, loss)));
	let mut stderr = io::stderr().lock();
	for (entry, loss) in details.take(MAX_LOSS_DETAILS) {
		let _ = writeln!(
			stderr,
			"Metadata loss at {:?} ({} other aliases), {:?} ({:?})",
			EscapedText(entry.path.as_str()),
			entry.aliases.len(),
			loss.field,
			loss.reason,
		);
	}
	if total > MAX_LOSS_DETAILS {
		let _ = writeln!(
			stderr,
			"{} additional metadata losses",
			total - MAX_LOSS_DETAILS
		);
	}
	total
}

#[derive(Default)]
pub(crate) struct Progress {
	read_step: Cell<u64>,
}

impl Progress {
	pub(crate) fn update(&self, event: BuildProgress) {
		let mut stderr = io::stderr().lock();
		let _ = match event {
			BuildProgress::Discovering => writeln!(stderr, "Discovering source paths"),
			BuildProgress::SourcesDiscovered {
				entries,
				regular_payload_bytes,
			} => writeln!(
				stderr,
				"Resolved {entries} entries with {regular_payload_bytes} source bytes"
			),
			BuildProgress::FinderAssetsPrepared { assets } => {
				writeln!(stderr, "Prepared {assets} Finder assets")
			}
			BuildProgress::WritingImage { logical_bytes } => {
				writeln!(stderr, "Encoding {logical_bytes} logical bytes")
			}
			BuildProgress::ReadingImage {
				logical_bytes_read,
				logical_bytes,
			} => {
				let step = logical_bytes_read
					.saturating_mul(READ_PROGRESS_STEPS)
					.checked_div(logical_bytes)
					.unwrap_or(0);
				if step <= self.read_step.get() {
					return;
				}
				self.read_step.set(step);
				writeln!(
					stderr,
					"Read {logical_bytes_read} of {logical_bytes} logical bytes"
				)
			}
			BuildProgress::VerifyingOutput => writeln!(stderr, "Verifying the staged image"),
			BuildProgress::Publishing => writeln!(stderr, "Checking sources before publication"),
			BuildProgress::Completed { physical_bytes } => {
				writeln!(stderr, "Published {physical_bytes} bytes")
			}
			_ => Ok(()),
		};
	}
}
