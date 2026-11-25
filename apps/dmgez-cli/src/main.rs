use std::{error::Error, num::NonZeroU32, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};

mod analyze_command;
mod capabilities;
#[cfg(feature = "convert")]
mod convert;
mod diff;
#[cfg(any(feature = "image-edit", feature = "project-edit"))]
mod edit_input;
mod extract;
mod image_edit;
#[cfg(feature = "appdmg-import")]
mod import_appdmg;
#[cfg(feature = "inspect")]
mod inspect;
mod inspect_command;
mod install;
mod installer_test;
#[cfg(feature = "lint")]
mod lint;
mod mount_command;
mod native_assessment;
mod notarize;
#[cfg(feature = "convert")]
mod optimize;
mod project_build;
mod project_edit;
#[cfg(any(feature = "appdmg-import", feature = "lint", feature = "project-build"))]
mod project_input;
#[cfg(any(
	feature = "hfsplus-read",
	feature = "apfs-read",
	feature = "signing-rust",
	feature = "signing-native",
	feature = "project-release",
	feature = "project-edit",
	feature = "image-edit",
	all(feature = "mount-system", target_os = "macos"),
	all(feature = "installer-lint", any(unix, windows)),
))]
mod report_output;
mod signing;
#[cfg(any(
	feature = "image-edit",
	feature = "project-edit",
	feature = "project-release"
))]
mod staged_image;
mod staple;
mod verify_command;
mod volume_commands;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod volume_input;
#[cfg(feature = "build")]
mod wrap;

const WRAP_RAW_COMMAND: &str = "wrap-raw";
const BUILD_PROJECT_COMMAND: &str = "build";
const BUILD_RELEASE_CAPABILITY: &str = "project-release";
const INSPECT_COMMAND: &str = "inspect";
const ANALYZE_COMMAND: &str = "analyze";
const CONVERT_COMMAND: &str = "convert";
const IMPORT_APPDMG_COMMAND: &str = "import-appdmg";
#[cfg(not(feature = "appdmg-import"))]
const APPDMG_IMPORT_FEATURE: &str = "appdmg-import";
const LINT_COMMAND: &str = "lint";
const SUPPRESSION_VALUE_NAME: &str = "RULE=REASON";
const LAYOUT_PROFILE_NAME: &str = "layout";
const INSTALLER_PROFILE_NAME: &str = "installer";
const INSTALLER_LINT_FEATURE: &str = "installer-lint";
const INSTALLER_LINT_COMPILED: bool = cfg!(all(feature = "installer-lint", any(unix, windows)));
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
const VERIFY_APFS_CAPABILITY: &str = "verify-apfs";
const HFSPLUS_DECMPFS_CAPABILITY: &str = "hfsplus-decmpfs";
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
	/// Apply a typed edit batch to a project and rebuild to a new image.
	Edit(project_edit::Arguments),
	/// Apply a typed edit batch to an existing HFS+ image and publish a fresh image.
	EditImage(image_edit::Arguments),
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
	/// Convert a qualified image to unsigned UDRO or UDZO.
	#[command(name = CONVERT_COMMAND)]
	Convert {
		image: PathBuf,
		#[arg(short, long)]
		output: PathBuf,
		#[arg(long, value_enum)]
		format: ConversionFormat,
		/// Allow discarding the source signature and any embedded notarization ticket.
		#[arg(long)]
		allow_signature_loss: bool,
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
	/// Compare decoded disk bytes, stored container bytes, or filesystem contents.
	#[command(name = diff::COMMAND)]
	Diff(diff::Arguments),
	/// Check project layout, installer inputs, or macOS policy for a release artifact.
	#[command(name = LINT_COMMAND)]
	Lint {
		project: PathBuf,
		#[arg(long, value_enum)]
		profile: LintProfile,
		/// Virtual folder for the layout profile. The default selects the root.
		#[arg(long)]
		folder: Option<String>,
		/// Suppress a rule with an explicit reason. May be repeated.
		#[arg(long, value_name = SUPPRESSION_VALUE_NAME)]
		suppress: Vec<String>,
		#[arg(long)]
		json: bool,
	},
	/// Inspect or verify a code signature, or sign an application or disk image.
	Sign {
		#[command(subcommand)]
		action: signing::Action,
	},
	/// Submit a signed disk image for notarization or check a submission's status.
	Notarize(notarize::Arguments),
	/// Attach and validate a notarization ticket on a new disk image copy.
	Staple(staple::Arguments),
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
	Inspect(inspect_command::Arguments),
	/// Measure run space and optionally check HFS+ allocation and file sizes.
	#[command(name = ANALYZE_COMMAND)]
	Analyze(analyze_command::Arguments),
	/// Check container integrity and, when requested, filesystem integrity.
	Verify(verify_command::Arguments),
	/// Test the finished installer image and explicitly requested release checks.
	Test(installer_test::Arguments),
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
	/// Copy one selected application bundle to an existing directory without mounting the image.
	#[command(name = install::COMMAND)]
	Install(install::Arguments),
	/// Attach an explicitly selected image read-only, or manage a dmgez mount session.
	Mount(mount_command::Arguments),
	/// Detach one verified dmgez mount session without forcing it.
	Unmount(mount_command::UnmountArguments),
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LintProfile {
	#[value(name = LAYOUT_PROFILE_NAME)]
	Layout,
	#[value(name = INSTALLER_PROFILE_NAME)]
	Installer,
	#[value(name = native_assessment::PROFILE)]
	MacosRelease,
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
	let error_status = if matches!(
		command,
		Command::Diff(_)
			| Command::Lint { .. }
			| Command::Sign { .. }
			| Command::Notarize(_)
			| Command::Staple(_)
			| Command::Test(_)
	) {
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
			capabilities::write();
			Ok(())
		}
		#[cfg(feature = "project-build")]
		Command::Build(arguments) => project_build::build(arguments, &std::env::current_dir()?),
		#[cfg(not(feature = "project-build"))]
		Command::Build(_) => Err("project building requires an HFS+ composed CLI".into()),
		Command::Edit(arguments) => project_edit::run(&arguments),
		Command::EditImage(arguments) => image_edit::run(&arguments),
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
			allow_signature_loss,
			encoding,
		} => convert::run(&image, &output, format, &encoding, allow_signature_loss),
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
		Command::Diff(arguments) => return diff::run(&arguments),
		#[cfg(feature = "lint")]
		Command::Lint {
			project,
			profile: LintProfile::Layout,
			folder,
			suppress,
			json,
		} => return lint::run(&project, folder.as_deref(), &suppress, json),
		#[cfg(all(feature = "installer-lint", any(unix, windows)))]
		Command::Lint {
			project,
			profile: LintProfile::Installer,
			folder,
			suppress,
			json,
		} => return lint::installer::run(&project, folder.as_deref(), &suppress, json),
		#[cfg(all(feature = "installer-lint", not(any(unix, windows))))]
		Command::Lint {
			profile: LintProfile::Installer,
			..
		} => Err("installer lint requires host source discovery".into()),
		#[cfg(not(feature = "installer-lint"))]
		Command::Lint {
			profile: LintProfile::Installer,
			..
		} => Err(format!("feature not compiled: {INSTALLER_LINT_FEATURE}").into()),
		#[cfg(not(feature = "lint"))]
		Command::Lint {
			profile: LintProfile::Layout,
			..
		} => Err(format!("feature not compiled: {LINT_COMMAND}").into()),
		Command::Lint {
			project,
			profile: LintProfile::MacosRelease,
			folder,
			suppress,
			json,
		} => return native_assessment::run(&project, folder.as_deref(), &suppress, json),
		Command::Sign { action } => return signing::run(&action),
		Command::Notarize(arguments) => return notarize::run(&arguments),
		Command::Staple(arguments) => staple::run(&arguments),
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
		Command::Inspect(arguments) => inspect_command::run(&arguments),
		Command::Analyze(arguments) => analyze_command::run(&arguments),
		#[cfg(feature = "inspect")]
		Command::Verify(arguments) => verify_command::run(&arguments),
		Command::Ls(arguments) => volume_commands::run(volume_commands::Query::List(&arguments)),
		Command::Tree(arguments) => volume_commands::run(volume_commands::Query::Tree(&arguments)),
		Command::Stat(arguments) => volume_commands::run(volume_commands::Query::Stat(&arguments)),
		Command::Cat(arguments) => volume_commands::run(volume_commands::Query::Cat(&arguments)),
		Command::Volumes(arguments) => volume_commands::volumes(&arguments),
		Command::Extract(arguments) => extract::run(&arguments),
		Command::Install(arguments) => install::run(&arguments),
		Command::Mount(arguments) => mount_command::run(&arguments),
		Command::Unmount(arguments) => mount_command::unmount(&arguments),
		Command::Test(arguments) => return installer_test::run(&arguments),
		#[cfg(feature = "inspect")]
		Command::UnwrapRaw {
			image,
			output,
			partition,
		} => inspect::unwrap(&image, &output, partition),
		#[cfg(not(feature = "inspect"))]
		Command::Verify(_) | Command::UnwrapRaw { .. } => Err("feature not compiled: inspect".into()),
	};
	result.map(|()| ExitCode::SUCCESS)
}
