use std::{error::Error, fmt::Display, num::NonZeroU32, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[cfg(feature = "convert")]
mod convert;
#[cfg(feature = "inspect")]
mod inspect;
#[cfg(feature = "build")]
mod wrap;

const WRAP_RAW_COMMAND: &str = "wrap-raw";
const INSPECT_COMMAND: &str = "inspect";
const ANALYZE_COMMAND: &str = "analyze";
const CONVERT_COMMAND: &str = "convert";
const UNWRAP_RAW_COMMAND: &str = "unwrap-raw";
const THREADS_ARG_ID: &str = "threads";
const THREADS_VALUE_NAME: &str = "THREADS";
const PARALLEL_FEATURE: &str = "parallel";
const LZFSE_FEATURE: &str = "lzfse";
const LZMA_FEATURE: &str = "lzma";
const VERIFY_CONTAINER_CAPABILITY: &str = "verify-container";
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
	/// Check UDIF integrity at the requested scope.
	Verify {
		image: PathBuf,
		#[arg(long)]
		json: bool,
		#[command(flatten)]
		scope: VerificationFlags,
	},
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

#[derive(Args)]
struct EncodingFlags {
	/// Encode on the calling thread even when parallel support is available.
	#[arg(long, conflicts_with = THREADS_ARG_ID)]
	serial: bool,
	/// Select a worker count. Zero selects a bounded automatic count.
	#[arg(long, id = THREADS_ARG_ID, value_name = THREADS_VALUE_NAME)]
	threads: Option<usize>,
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

type CliResult = Result<(), Box<dyn Error>>;

fn main() -> ExitCode {
	match run(Cli::parse().command) {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("error: {error}");
			let mut cause = error.source();
			while let Some(inner) = cause {
				eprintln!("  caused by: {inner}");
				cause = inner.source();
			}
			ExitCode::FAILURE
		}
	}
}

fn run(command: Command) -> CliResult {
	match command {
		Command::Capabilities => {
			for (name, compiled) in [
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
						any(feature = "build", feature = "inspect")
					)),
				),
				(
					LZMA_FEATURE,
					cfg!(all(
						feature = "lzma",
						any(feature = "build", feature = "inspect")
					)),
				),
				(INSPECT_COMMAND, cfg!(feature = "inspect")),
				(ANALYZE_COMMAND, cfg!(feature = "inspect")),
				(CONVERT_COMMAND, cfg!(feature = "convert")),
				(VERIFY_CONTAINER_CAPABILITY, cfg!(feature = "inspect")),
				(UNWRAP_RAW_COMMAND, cfg!(feature = "inspect")),
			] {
				report_field(name, if compiled { COMPILED } else { NOT_COMPILED });
			}
			Ok(())
		}
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
		Command::Convert { .. } => Err(format!("feature not compiled: {CONVERT_COMMAND}").into()),
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
	}
}

fn report_field(key: impl Display, value: impl Display) {
	println!("{key} = {value}");
}
