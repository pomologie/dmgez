use std::{
	cell::Cell,
	collections::BTreeMap,
	fs::File,
	io::{self, Read, Write},
	path::{Path, PathBuf},
	thread,
};

use clap::Args;
use dmgez_builder::{
	planning::PreparedBuild,
	preflight::{OutputIntent, PreflightOptions, ReleaseIntent},
	progress::BuildProgress,
};
use dmgez_config::{ConfigError, ConfigOverride, MAX_MANIFEST_BYTES, OverrideError, Project};
use dmgez_hfs_adapter_probe::hfsplus::{self, HfsImage, MetadataLoss};
use thiserror::Error;

const DEFINITION_SEPARATOR: char = '=';
const OVERWRITE_ASSIGNMENT: &str = "build.overwrite=true";
const MAX_LOSS_DETAILS: usize = 20;
const READ_PROGRESS_STEPS: u64 = 10;

#[derive(Args)]
pub(crate) struct Arguments {
	/// TOML project file. Relative paths use the caller's working directory.
	project: PathBuf,
	/// Plan the exact virtual disk capacity and report the resolved build summary.
	#[arg(long)]
	dry_run: bool,
	/// Define a literal substitution value. May be repeated.
	#[arg(long, value_name = "NAME=VALUE")]
	define: Vec<String>,
	/// Override a typed scalar project setting. May be repeated.
	#[arg(long, value_name = "SECTION.FIELD=VALUE")]
	set: Vec<String>,
	/// Authorize staged replacement in a trusted local output directory.
	#[arg(long)]
	overwrite: bool,
}

#[derive(Debug, Error)]
pub(crate) enum Error {
	#[error("working directory must be absolute")]
	RelativeWorkingDirectory,
	#[error("project path has no parent directory")]
	MissingProjectDirectory,
	#[error("expected a definition in NAME=VALUE form")]
	Definition,
	#[error("choose either --overwrite or --set build.overwrite")]
	DuplicateOverwrite,
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Override(#[from] OverrideError),
	#[error(transparent)]
	Config(#[from] ConfigError),
	#[error(transparent)]
	Build(#[from] hfsplus::Error),
}

pub(crate) fn build(arguments: Arguments, working_directory: &Path) -> Result<(), Error> {
	if !working_directory.is_absolute() {
		return Err(Error::RelativeWorkingDirectory);
	}
	let path = working_directory.join(&arguments.project).canonicalize()?;
	let directory = path.parent().ok_or(Error::MissingProjectDirectory)?;
	let mut input = String::new();
	File::open(&path)?
		.take(MAX_MANIFEST_BYTES as u64 + 1)
		.read_to_string(&mut input)?;
	let mut defines = BTreeMap::new();
	for definition in arguments.define {
		let (name, value) = definition
			.split_once(DEFINITION_SEPARATOR)
			.ok_or(Error::Definition)?;
		defines.insert(name.to_owned(), value.to_owned());
	}
	let mut overrides = arguments
		.set
		.iter()
		.map(|assignment| ConfigOverride::parse(assignment))
		.collect::<Result<Vec<_>, _>>()?;
	if arguments.overwrite {
		let overwrite = ConfigOverride::parse(OVERWRITE_ASSIGNMENT)?;
		if overrides
			.iter()
			.any(|assignment| assignment.key() == overwrite.key())
		{
			return Err(Error::DuplicateOverwrite);
		}
		overrides.push(overwrite);
	}
	let project = Project::parse_with_overrides(&input, &defines, &overrides, working_directory)?;
	let progress = Progress::default();
	let prepared = hfsplus::prepare_with_progress(
		&project,
		directory,
		PreflightOptions {
			capabilities: hfsplus::capabilities(),
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: thread::available_parallelism()?,
		},
		Default::default(),
		|event| progress.update(event),
	)?;
	let sealed = prepared.seal()?;
	let losses = report_losses(sealed.metadata_losses());
	if arguments.dry_run {
		write_dry_run(io::stdout().lock(), prepared.prepared(), &sealed, losses)?;
		return Ok(());
	}
	let report = sealed.publish_with_progress(None, |event| progress.update(event))?;
	let image = report.report.image;
	writeln!(
		io::stdout().lock(),
		"{} logical bytes, {} physical bytes, {losses} metadata losses",
		image.logical_bytes,
		image.physical_bytes,
	)?;
	Ok(())
}

fn write_dry_run(
	mut output: impl Write,
	prepared: &PreparedBuild,
	image: &HfsImage<'_>,
	losses: usize,
) -> io::Result<()> {
	let profile = prepared.profile();
	let manifest = prepared.manifest();
	#[cfg(feature = "finder-write")]
	let finder_assets = prepared.finder_assets().len();
	#[cfg(not(feature = "finder-write"))]
	let finder_assets = 0;
	writeln!(output, "mode=dry-run")?;
	writeln!(output, "format={:?}", profile.write_options().profile)?;
	writeln!(output, "filesystem={:?}", profile.filesystem())?;
	writeln!(output, "finder={:?}", profile.finder())?;
	writeln!(output, "output={:?}", prepared.output_path())?;
	writeln!(output, "publication={:?}", profile.output())?;
	writeln!(output, "release={:?}", profile.release())?;
	writeln!(output, "logical_bytes={}", image.logical_bytes())?;
	writeln!(output, "physical_bytes=unknown")?;
	writeln!(output, "resolved_entries={}", manifest.entries().len())?;
	writeln!(
		output,
		"source_payload_bytes={}",
		manifest.regular_payload_bytes()
	)?;
	writeln!(output, "finder_assets={finder_assets}")?;
	writeln!(output, "metadata_losses={losses}")?;
	Ok(())
}

fn report_losses(entries: &[MetadataLoss<'_>]) -> usize {
	let total = entries.iter().map(|entry| entry.details.len()).sum();
	let details = entries
		.iter()
		.flat_map(|entry| entry.details.iter().map(move |loss| (entry, loss)));
	let mut stderr = io::stderr().lock();
	for (entry, loss) in details.take(MAX_LOSS_DETAILS) {
		let _ = writeln!(
			stderr,
			"Metadata loss at {:?} ({} other aliases), {:?} ({:?})",
			entry.path.as_str(),
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
struct Progress {
	read_step: Cell<u64>,
}

impl Progress {
	fn update(&self, event: BuildProgress) {
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
