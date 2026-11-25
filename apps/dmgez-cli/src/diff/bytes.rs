use std::{
	fs::File,
	io::{self, Write},
	path::Path,
	process::ExitCode,
};

use dmgez_diff::{DEFAULT_COMPARE_BYTES, Outcome, Report, Side, compare};
use dmgez_udif::{
	Error as UdifError,
	reader::{FileSource, Image, Limits},
};
use dmgez_vfs::Raw;
use serde::Serialize;

use crate::CliResult;

use super::{Arguments, DIFFERENT_EXIT_CODE, require_regular_file, with_side};

const LOGICAL_SPACE: &str = "logical_disk";
const PHYSICAL_SPACE: &str = "physical_file";

#[derive(Serialize)]
struct DiffReport {
	address_space: &'static str,
	#[serde(flatten)]
	comparison: Report,
}

pub(super) fn run(arguments: &Arguments, logical: bool) -> CliResult<ExitCode> {
	let left = with_side(open_file(&arguments.left), Side::Left)?;
	let right = with_side(open_file(&arguments.right), Side::Right)?;
	let max_bytes = arguments.max_bytes.unwrap_or(DEFAULT_COMPARE_BYTES);
	let (address_space, comparison) = if logical {
		let limits = Limits::default();
		let mut left = with_side(Image::open(left, limits), Side::Left)?;
		let mut right = with_side(Image::open(right, limits), Side::Right)?;
		(
			LOGICAL_SPACE,
			compare(&mut left, &mut right, max_bytes, || false)?,
		)
	} else {
		(
			PHYSICAL_SPACE,
			compare(&mut Raw(left), &mut Raw(right), max_bytes, || false)?,
		)
	};
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

fn open_file(path: &Path) -> Result<FileSource, UdifError> {
	require_regular_file(path)?;
	FileSource::new(File::open(path)?)
}
