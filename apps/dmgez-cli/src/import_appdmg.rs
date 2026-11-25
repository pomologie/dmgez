use std::{
	io::{self, Write},
	path::{Path, absolute},
};

use dmgez_config::import_appdmg;
use dmgez_core::output::publish_new;

use crate::{CliResult, project_input};

pub(crate) fn run(input: &Path, output: &Path, image_output: &str) -> CliResult {
	let input = absolute(input)?;
	let directory = input
		.parent()
		.ok_or("appdmg settings require a parent directory")?;
	let source = project_input::read(&input)?;
	let imported = import_appdmg(&source, directory, image_output)?;
	let mut stderr = io::stderr().lock();
	for diagnostic in &imported.diagnostics {
		writeln!(stderr, "warning: {diagnostic}")?;
	}
	publish_new(
		output,
		|file| file.write_all(imported.manifest.as_bytes()),
		|_, ()| Ok::<(), io::Error>(()),
	)?;
	Ok(())
}
