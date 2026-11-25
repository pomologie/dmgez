use std::{error::Error, fmt::Display, num::NonZeroU32, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[cfg(feature = "convert")]
mod convert;
#[cfg(feature = "diff")]
mod diff;
mod extract;
#[cfg(feature = "appdmg-import")]
mod import_appdmg;
#[cfg(feature = "inspect")]
mod inspect;
#[cfg(feature = "lint")]
mod lint;
#[cfg(feature = "convert")]
mod optimize;
mod project_build;
#[cfg(any(feature = "appdmg-import", feature = "lint", feature = "project-build"))]
mod project_input;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod report_output;
mod volume_commands;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod volume_input;
#[cfg(feature = "build")]
mod wrap;

const WRAP_RAW_COMMAND: &str = "wrap-raw";
const BUILD_PROJECT_COMMAND: &str = "build";
const INSPECT_COMMAND: &str = "inspect";
const ANALYZE_COMMAND: &str = "analyze";
const CONVERT_COMMAND: &str = "convert";
const DIFF_COMMAND: &str = "diff";
const IMPORT_APPDMG_COMMAND: &str = "import-appdmg";
#[cfg(not(feature = "appdmg-import"))]
const APPDMG_IMPORT_FEATURE: &str = "appdmg-import";
const LINT_COMMAND: &str = "lint";
const SUPPRESSION_VALUE_NAME: &str = "RULE=REASON";
const LAYOUT_PROFILE_NAME: &str = "layout";
const REPORT_ERROR_EXIT_CODE: u8 = 2;
const OPTIMIZE_COMMAND: &str = "optimize";
const UNWRAP_RAW_COMMAND: &str = "unwrap-raw";
const THREADS_ARG_ID: &str = "threads";
const THREADS_VALUE_NAME: &str = "THREADS";
const PARALLEL_FEATURE: &str = "parallel";
const LZFSE_FEATURE: &str = "lzfse";
const LZMA_FEATURE: &str = "lzma";
const VERIFY_CONTAINER_CAPABILITY: &str = "verify-container";
const VERIFY_HFSPLUS_CAPABILITY: &str = "verify-hfsplus";
const HFSPLUS_DECMPFS_CAPABILITY: &str = "hfsplus-decmpfs";
const RUN_LIMIT_ARG: &str = "run_limit";
const PARTITION_SLOT_VALUE: &str = "SLOT";
const COMPILED: &str = "compiled";
const NOT_COMPILED: &str = "not_compiled";

#[derive(Parser)]
#[command(name = env!("CARGO_BIN_NAME"), version, about = "Create and inspect macOS disk images")]
struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	/// List the capabilities built into this binary.
	Capabilities,
	/// Plan or build an HFS+ disk image from a TOML project.
	#[command(name = BUILD_PROJECT_COMMAND)]
	Build(project_build::Arguments),
	/// Wrap a regular file of raw sectors in a UDIF image.
	#[command(name = WRAP_RAW_COMMAND)]
	WrapRaw {
		source: PathBuf,
		#[arg(short, long)]
		output: PathBuf,
		#[arg(long, value_enum, default_value_t)]
		format: Format,
		#[command(flatten)]
		encoding: EncodingFlags,
	},
	/// Convert a qualified unsigned image between UDRO and UDZO.
	#[command(name = CONVERT_COMMAND)]
	Convert {
		image: PathBuf,
		#[arg(short, long)]
		output: PathBuf,
		#[arg(long, value_enum)]
		format: ConversionFormat,
		#[command(flatten)]
		encoding: EncodingFlags,
	},
	/// Sample eligible unsigned profiles and publish a verified conversion.
	#[command(name = OPTIMIZE_COMMAND)]
	Optimize {
		image: PathBuf,
		#[arg(short, long)]
		output: PathBuf,
		#[command(flatten)]
		sampling: SamplingFlags,
		#[command(flatten)]
		encoding: EncodingFlags,
	},
	/// Compare decoded disk bytes or stored container bytes.
	#[command(name = DIFF_COMMAND)]
	Diff {
		left: PathBuf,
		right: PathBuf,
		#[arg(long, value_enum, default_value_t)]
		mode: DiffMode,
		/// Maximum paired bytes compared from each input.
		#[arg(long)]
		max_bytes: Option<u64>,
		#[arg(long)]
		json: bool,
	},
	/// Check configured Finder icon geometry in a project folder.
	#[command(name = LINT_COMMAND)]
	Lint {
		project: PathBuf,
		#[arg(long, value_enum)]
		profile: LintProfile,
		/// Virtual project folder to inspect. The default selects the root.
		#[arg(long)]
		folder: Option<String>,
		/// Suppress a rule with an explicit reason. May be repeated.
		#[arg(long, value_name = SUPPRESSION_VALUE_NAME)]
		suppress: Vec<String>,
		#[arg(long)]
		json: bool,
	},
	/// Translate appdmg JSON settings into a new dmgez project.
	#[command(name = IMPORT_APPDMG_COMMAND)]
	ImportAppdmg {
		settings: PathBuf,
		/// Destination for the generated TOML manifest.
		#[arg(short, long)]
		output: PathBuf,
		/// Image output to record in the manifest, relative to that manifest if not absolute.
		#[arg(long)]
		image_output: String,
	},
	/// Show UDIF metadata and optionally probe the decoded partition map.
	#[command(name = INSPECT_COMMAND)]
	Inspect {
		image: PathBuf,
		#[arg(long)]
		json: bool,
		/// Report logical disk partitions or recognized partitionless geometry.
		#[arg(long)]
		partitions: bool,
		/// First logical run in the optional detail page.
		#[arg(long, requires = RUN_LIMIT_ARG)]
		run_offset: Option<usize>,
		/// Maximum number of runs in the optional detail page.
		#[arg(long, id = RUN_LIMIT_ARG)]
		run_limit: Option<usize>,
	},
	/// Measure stored and logical byte counts from a disk image's run map.
	#[command(name = ANALYZE_COMMAND)]
	Analyze {
		image: PathBuf,
		#[arg(long)]
		json: bool,
	},
	/// Check container integrity and, when requested, filesystem integrity.
	Verify {
		image: PathBuf,
		#[arg(long)]
		json: bool,
		#[command(flatten)]
		scope: VerificationFlags,
	},
	/// List a page of visible directory entries in a disk image.
	Ls(volume_commands::ListArguments),
	/// Browse a directory tree in a disk image.
	Tree(volume_commands::TreeArguments),
	/// Report metadata for one path in a disk image.
	Stat(volume_commands::StatArguments),
	/// Write the selected file fork to standard output.
	Cat(volume_commands::CatArguments),
	/// List APFS volume identities at a retained container checkpoint.
	#[command(name = volume_commands::VOLUMES_COMMAND)]
	Volumes(volume_commands::VolumesArguments),
	/// Extract a selected fork or directory tree to an absent host path.
	Extract(extract::Arguments),
	/// Export the decoded disk or one partition to a regular file.
	#[command(name = UNWRAP_RAW_COMMAND)]
	UnwrapRaw {
		image: PathBuf,
		#[arg(short, long)]
		output: PathBuf,
		/// Export the complete extent of a one-based slot from `inspect --partitions`.
		#[arg(long, value_name = PARTITION_SLOT_VALUE)]
		partition: Option<NonZeroU32>,
	},
}

#[derive(Args)]
#[group(multiple = false)]
struct VerificationFlags {
	/// Check container structure without reading payloads.
	#[arg(long)]
	quick: bool,
	/// Check structure, payload, and all declared container CRCs. This is the default.
	#[arg(long)]
	container: bool,
	/// Request both container and filesystem verification.
	#[arg(long)]
	full: bool,
}

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
enum Format {
	Udro,
	#[default]
	Udzo,
	Ulfo,
	Ulmo,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ConversionFormat {
	Udro,
	Udzo,
}

#[derive(Debug, Default, Clone, Copy, ValueEnum)]
enum DiffMode {
	#[default]
	Logical,
	Physical,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LintProfile {
	#[value(name = LAYOUT_PROFILE_NAME)]
	Layout,
}

#[derive(Args)]
struct EncodingFlags {
	/// Encode on the calling thread even when parallel support is available.
	#[arg(long, conflicts_with = THREADS_ARG_ID)]
	serial: bool,
	/// Select a worker count. Zero selects a bounded automatic count.
	#[arg(long, id = THREADS_ARG_ID, value_name = THREADS_VALUE_NAME)]
	threads: Option<usize>,
}

#[derive(Args)]
struct SamplingFlags {
	/// Maximum whole output chunks to sample.
	#[arg(long)]
	max_sample_chunks: Option<u16>,
	/// Maximum logical sample bytes supplied to each candidate.
	#[arg(long)]
	max_sample_bytes: Option<u64>,
}

#[cfg(any(feature = "build", feature = "convert"))]
impl EncodingFlags {
	fn execution(&self) -> Result<dmgez_udif::writer::Execution, dmgez_udif::Error> {
		use dmgez_udif::{Error, writer::Execution};

		const SERIAL_WORKER_COUNT: usize = 1;
		match (self.serial, cfg!(feature = "parallel"), self.threads) {
			(true, _, _)
			| (false, false, None | Some(Execution::AUTO_WORKERS | SERIAL_WORKER_COUNT)) => {
				Ok(Execution::Serial)
			}
			(false, false, Some(_)) => Err(Error::FeatureNotCompiled(PARALLEL_FEATURE)),
			(false, true, workers) => Ok(Execution::Parallel {
				workers: workers.unwrap_or(Execution::AUTO_WORKERS),
			}),
		}
	}
}

type CliResult<T = ()> = Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
	let command = Cli::parse().command;
	let error_status = if matches!(command, Command::Diff { .. } | Command::Lint { .. }) {
		ExitCode::from(REPORT_ERROR_EXIT_CODE)
	} else {
		ExitCode::FAILURE
	};
	match run(command) {
		Ok(status) => status,
		Err(error) => {
			eprintln!("error: {error}");
			let mut cause = error.source();
			while let Some(inner) = cause {
				eprintln!("  caused by: {inner}");
				cause = inner.source();
			}
			error_status
		}
	}
}

fn run(command: Command) -> CliResult<ExitCode> {
	let result = match command {
		Command::Capabilities => {
			for (name, compiled) in [
				(BUILD_PROJECT_COMMAND, cfg!(feature = "project-build")),
				(WRAP_RAW_COMMAND, cfg!(feature = "build")),
				(
					PARALLEL_FEATURE,
					cfg!(all(
						any(feature = "build", feature = "convert"),
						feature = "parallel"
					)),
				),
				(
					LZFSE_FEATURE,
					cfg!(all(
						feature = "lzfse",
						any(feature = "build", feature = "inspect", feature = "diff")
					)),
				),
				(
					LZMA_FEATURE,
					cfg!(all(
						feature = "lzma",
						any(feature = "build", feature = "inspect", feature = "diff")
					)),
				),
				(INSPECT_COMMAND, cfg!(feature = "inspect")),
				(VERIFY_HFSPLUS_CAPABILITY, cfg!(feature = "hfsplus-verify")),
				(
					HFSPLUS_DECMPFS_CAPABILITY,
					cfg!(feature = "hfsplus-decmpfs"),
				),
				(
					volume_commands::HFSPLUS_READ_FEATURE,
					cfg!(feature = "hfsplus-read"),
				),
				(
					volume_commands::APFS_READ_FEATURE,
					cfg!(feature = "apfs-read"),
				),
				(extract::FEATURE, extract::COMPILED),
				(ANALYZE_COMMAND, cfg!(feature = "inspect")),
				(CONVERT_COMMAND, cfg!(feature = "convert")),
				(OPTIMIZE_COMMAND, cfg!(feature = "convert")),
				(DIFF_COMMAND, cfg!(feature = "diff")),
				(IMPORT_APPDMG_COMMAND, cfg!(feature = "appdmg-import")),
				(LINT_COMMAND, cfg!(feature = "lint")),
				(VERIFY_CONTAINER_CAPABILITY, cfg!(feature = "inspect")),
				(UNWRAP_RAW_COMMAND, cfg!(feature = "inspect")),
			] {
				report_field(name, if compiled { COMPILED } else { NOT_COMPILED });
			}
			Ok(())
		}
		#[cfg(feature = "project-build")]
		Command::Build(arguments) => project_build::build(arguments, &std::env::current_dir()?),
		#[cfg(not(feature = "project-build"))]
		Command::Build(_) => Err("project building requires an HFS+ composed CLI".into()),
		#[cfg(feature = "build")]
		Command::WrapRaw {
			source,
			output,
			format,
			encoding,
		} => wrap::run(&source, &output, format, &encoding),
		#[cfg(not(feature = "build"))]
		Command::WrapRaw { .. } => Err("feature not compiled: build".into()),
		#[cfg(feature = "convert")]
		Command::Convert {
			image,
			output,
			format,
			encoding,
		} => convert::run(&image, &output, format, &encoding),
		#[cfg(not(feature = "convert"))]
		Command::Convert { .. } | Command::Optimize { .. } => {
			Err(format!("feature not compiled: {CONVERT_COMMAND}").into())
		}
		#[cfg(feature = "convert")]
		Command::Optimize {
			image,
			output,
			sampling,
			encoding,
		} => optimize::run(&image, &output, sampling, &encoding),
		#[cfg(feature = "diff")]
		Command::Diff {
			left,
			right,
			mode,
			max_bytes,
			json,
		} => return diff::run(&left, &right, mode, max_bytes, json),
		#[cfg(not(feature = "diff"))]
		Command::Diff { .. } => Err(format!("feature not compiled: {DIFF_COMMAND}").into()),
		#[cfg(feature = "lint")]
		Command::Lint {
			project,
			profile: LintProfile::Layout,
			folder,
			suppress,
			json,
		} => return lint::run(&project, folder.as_deref(), &suppress, json),
		#[cfg(not(feature = "lint"))]
		Command::Lint { .. } => Err(format!("feature not compiled: {LINT_COMMAND}").into()),
		#[cfg(feature = "appdmg-import")]
		Command::ImportAppdmg {
			settings,
			output,
			image_output,
		} => import_appdmg::run(&settings, &output, &image_output),
		#[cfg(not(feature = "appdmg-import"))]
		Command::ImportAppdmg { .. } => {
			Err(format!("feature not compiled: {APPDMG_IMPORT_FEATURE}").into())
		}
		#[cfg(feature = "inspect")]
		Command::Inspect {
			image,
			json,
			partitions,
			run_offset,
			run_limit,
		} => inspect::show(&image, json, partitions, run_offset.unwrap_or(0), run_limit),
		#[cfg(feature = "inspect")]
		Command::Analyze { image, json } => inspect::analyze(&image, json),
		#[cfg(feature = "inspect")]
		Command::Verify { image, json, scope } => inspect::verify(&image, json, scope),
		Command::Ls(arguments) => volume_commands::run(volume_commands::Query::List(&arguments)),
		Command::Tree(arguments) => volume_commands::run(volume_commands::Query::Tree(&arguments)),
		Command::Stat(arguments) => volume_commands::run(volume_commands::Query::Stat(&arguments)),
		Command::Cat(arguments) => volume_commands::run(volume_commands::Query::Cat(&arguments)),
		Command::Volumes(arguments) => volume_commands::volumes(&arguments),
		Command::Extract(arguments) => extract::run(&arguments),
		#[cfg(feature = "inspect")]
		Command::UnwrapRaw {
			image,
			output,
			partition,
		} => inspect::unwrap(&image, &output, partition),
		#[cfg(not(feature = "inspect"))]
		Command::Inspect { .. }
		| Command::Analyze { .. }
		| Command::Verify { .. }
		| Command::UnwrapRaw { .. } => Err("feature not compiled: inspect".into()),
	};
	result.map(|()| ExitCode::SUCCESS)
}

fn report_field(key: impl Display, value: impl Display) {
	println!("{key} = {value}");
}
