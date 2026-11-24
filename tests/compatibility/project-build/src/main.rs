use std::{env, error::Error, path::PathBuf, process::ExitCode};

use clap::Parser;

mod project;

#[derive(Parser)]
#[command(about = "Build a TOML project through the HFS+ compatibility adapter")]
struct Cli {
	/// Caller directory preserved by the isolated Cargo runner.
	#[arg(long, hide = true)]
	working_directory: Option<PathBuf>,
	#[command(flatten)]
	build: project::Arguments,
}

fn main() -> ExitCode {
	let cli = Cli::parse();
	match run(cli) {
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

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
	let directory = match cli.working_directory {
		Some(directory) => directory,
		None => env::current_dir()?,
	};
	project::build(cli.build, &directory)?;
	Ok(())
}
