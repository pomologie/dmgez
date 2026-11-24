use std::{error::Error, fmt::Display, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[cfg(feature = "inspect")]
mod inspect;
#[cfg(feature = "build")]
mod wrap;

const WRAP_RAW_COMMAND: &str = "wrap-raw";
const INSPECT_COMMAND: &str = "inspect";
const ANALYZE_COMMAND: &str = "analyze";
const UNWRAP_RAW_COMMAND: &str = "unwrap-raw";
const THREADS_ARG_ID: &str = "threads";
const THREADS_VALUE_NAME: &str = "THREADS";
const PARALLEL_FEATURE: &str = "parallel";
const LZFSE_FEATURE: &str = "lzfse";
const LZMA_FEATURE: &str = "lzma";
const VERIFY_CONTAINER_CAPABILITY: &str = "verify-container";
const RUN_LIMIT_ARG: &str = "run_limit";
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
		/// Encode on the calling thread even when parallel support is available.
		#[arg(long, conflicts_with = THREADS_ARG_ID)]
		serial: bool,
		/// Select a worker count. Zero selects a bounded automatic count.
		#[arg(long, id = THREADS_ARG_ID, value_name = THREADS_VALUE_NAME)]
		threads: Option<usize>,
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
	/// Export the complete logical disk to a regular file.
	#[command(name = UNWRAP_RAW_COMMAND)]
	UnwrapRaw {
		image: PathBuf,
		#[arg(short, long)]
		output: PathBuf,
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
					cfg!(all(feature = "build", feature = "parallel")),
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
			serial,
			threads,
		} => wrap::run(&source, &output, format, serial, threads),
		#[cfg(not(feature = "build"))]
		Command::WrapRaw { .. } => Err("feature not compiled: build".into()),
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
		Command::UnwrapRaw { image, output } => inspect::unwrap(&image, &output),
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
