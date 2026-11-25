use std::{
	io::{self, Write},
	path::Path,
};

use dmgez_runtime::appdmg::ImportedProject;

use crate::CliResult;

pub(crate) fn run(input: &Path, output: &Path, image_output: &str) -> CliResult {
	let imported = ImportedProject::prepare(input, image_output)?;
	let mut stderr = io::stderr().lock();
	for diagnostic in imported.diagnostics() {
		writeln!(stderr, "warning: {diagnostic}")?;
	}
	imported.publish(output)?;
	Ok(())
}
