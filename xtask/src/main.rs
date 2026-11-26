use std::{error::Error, path::PathBuf, process::ExitCode};

use cargo_metadata::MetadataCommand;
use clap::{Args, Parser, Subcommand};

mod dependencies;
mod evidence;
mod registry_source;
mod source;

const MANIFEST: &str = "Cargo.toml";

#[derive(Parser)]
struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	/// Check structural Rust entry points, source size and CLI dependency profiles.
	Architecture(Architecture),
	/// Check the supplied release evidence against the clean candidate checkout.
	ReleaseEvidence(evidence::Arguments),
	/// Prepare a Rust source file for the standalone registry package.
	BundleSource(registry_source::Arguments),
}

#[derive(Args)]
struct Architecture {
	/// Cargo manifest for the workspace to check.
	#[arg(long, default_value = MANIFEST)]
	manifest_path: PathBuf,
	/// Check Rust sources without resolving CLI feature profiles.
	#[arg(long)]
	source_only: bool,
}

fn main() -> ExitCode {
	let result = match Cli::parse().command {
		Command::Architecture(arguments) => check(arguments),
		Command::ReleaseEvidence(arguments) => evidence::check(arguments),
		Command::BundleSource(arguments) => registry_source::rewrite(arguments),
	};
	match result {
		Ok(()) => ExitCode::SUCCESS,
		Err(error) => {
			eprintln!("{error}");
			ExitCode::FAILURE
		}
	}
}

fn check(arguments: Architecture) -> Result<(), Box<dyn Error>> {
	let metadata = MetadataCommand::new()
		.manifest_path(arguments.manifest_path)
		.no_deps()
		.other_options(vec!["--locked".into()])
		.exec()?;
	let root = metadata.workspace_root.as_std_path();
	let target = metadata.target_directory.as_std_path();
	let mut errors = Vec::new();
	match source::check(root, target) {
		Ok(roots) => {
			println!("Checked {roots} Rust library and module entry points and source size")
		}
		Err(error) => errors.push(error.to_string()),
	}
	if !arguments.source_only {
		match dependencies::check(root) {
			Ok(()) => println!("CLI dependency profiles passed"),
			Err(error) => errors.push(error.to_string()),
		}
	}
	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors.join("\n").into())
	}
}
