use std::{
	io::{self, Write},
	process::ExitCode,
};

use dmgez_diff::{DEFAULT_COMPARE_BYTES, Outcome, Report};
use dmgez_runtime::compare::{self, ByteSpace};
use serde::Serialize;

use crate::CliResult;

use super::{Arguments, DIFFERENT_EXIT_CODE};

const LOGICAL_SPACE: &str = "logical_disk";
const PHYSICAL_SPACE: &str = "physical_file";

#[derive(Serialize)]
struct DiffReport {
	address_space: &'static str,
	#[serde(flatten)]
	comparison: Report,
}

pub(super) fn run(arguments: &Arguments, logical: bool) -> CliResult<ExitCode> {
	let (address_space, space) = if logical {
		(LOGICAL_SPACE, ByteSpace::Logical(Default::default()))
	} else {
		(PHYSICAL_SPACE, ByteSpace::Physical)
	};
	let comparison = compare::files(
		&arguments.left,
		&arguments.right,
		space,
		arguments.max_bytes.unwrap_or(DEFAULT_COMPARE_BYTES),
		|| false,
	)?;
	let mut stdout = io::stdout().lock();
	if arguments.json {
		serde_json::to_writer(
			&mut stdout,
			&DiffReport {
				address_space,
				comparison,
			},
		)?;
		writeln!(stdout)?;
	} else {
		writeln!(stdout, "address_space = {address_space}")?;
		writeln!(stdout, "outcome = {}", comparison.outcome.as_str())?;
		writeln!(stdout, "left_bytes = {}", comparison.left_bytes)?;
		writeln!(stdout, "right_bytes = {}", comparison.right_bytes)?;
		writeln!(
			stdout,
			"common_prefix_bytes = {}",
			comparison.common_prefix_bytes
		)?;
	}
	let code = if comparison.outcome == Outcome::Equal {
		ExitCode::SUCCESS
	} else {
		ExitCode::from(DIFFERENT_EXIT_CODE)
	};
	Ok(code)
}
