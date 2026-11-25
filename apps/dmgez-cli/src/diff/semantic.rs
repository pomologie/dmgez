use std::{
	io::{self, Write},
	process::ExitCode,
};

#[cfg(feature = "apfs-read")]
use dmgez_diff::Side;
#[cfg(feature = "diff-hash")]
use dmgez_diff::semantic::Contents;
#[cfg(feature = "finder-diff")]
use dmgez_diff::semantic::Field;
use dmgez_diff::semantic::{Limits, Options, Outcome, PathMatching, Report};
use dmgez_inspector::SelectedWindowId;
#[cfg(feature = "apfs-read")]
use dmgez_runtime::volume::Error as VolumeError;
use dmgez_runtime::{
	compare::semantic::{self, ComparedInput},
	disk::Encoding,
};
use serde::Serialize;

#[cfg(feature = "apfs-read")]
use crate::volume_commands::{
	VolumeIdentity, checkpoint, volume_selector, with_named_selector_guidance,
};
use crate::{
	CliResult,
	report_output::{write_json, write_report},
	volume_commands::Filesystem,
};

#[cfg(feature = "diff-hash")]
use super::ContentsMode;
use super::{Arguments, DIFFERENT_EXIT_CODE, PathMode};
#[cfg(feature = "apfs-read")]
use super::{LEFT_VOLUME_OPTION, RIGHT_VOLUME_OPTION, with_side};

const FILESYSTEM_SPACE: &str = "filesystem";
const EQUIVALENT_PATH_SPELLING_LABEL: &str = "equivalent_path_spelling";
const RIGHT_PATH_LABEL: &str = "right_path";
#[cfg(feature = "diff-hash")]
const RENAME_CANDIDATE_LABEL: &str = "rename_candidate";

#[cfg(feature = "hfsplus-read")]
pub(super) fn hfsplus(arguments: &Arguments) -> CliResult<ExitCode> {
	let (left, right) = inputs(arguments);
	let (limits, options) = comparison_policy(arguments);
	let report = semantic::hfsplus(left, right, limits, options, || false)?;
	DiffReport::new(arguments, Filesystem::Hfsplus, report, |signature| {
		Volume::HfsSignature(signature.into())
	})
	.write(arguments.json)
}

#[cfg(feature = "apfs-read")]
pub(super) fn apfs(arguments: &Arguments) -> CliResult<ExitCode> {
	let options = &arguments.options;
	let left_volume = with_side(
		options
			.left_volume
			.as_deref()
			.map(volume_selector)
			.transpose(),
		Side::Left,
	)?;
	let right_volume = with_side(
		options
			.right_volume
			.as_deref()
			.map(volume_selector)
			.transpose(),
		Side::Right,
	)?;
	let (left, right) = inputs(arguments);
	let left = semantic::ApfsInput {
		disk: left,
		volume: left_volume,
		checkpoint: checkpoint(options.left_xid),
	};
	let right = semantic::ApfsInput {
		disk: right,
		volume: right_volume,
		checkpoint: checkpoint(options.right_xid),
	};
	let (limits, options) = comparison_policy(arguments);
	let report = match semantic::apfs(left, right, limits, options, || false) {
		Err(semantic::Error::Input(input)) => {
			let option = if input.side == Side::Left {
				LEFT_VOLUME_OPTION
			} else {
				RIGHT_VOLUME_OPTION
			};
			let error = match input.source {
				VolumeError::Apfs(error) => with_named_selector_guidance(error.into(), option),
				error => error.into(),
			};
			with_side(Err(error), input.side)?
		}
		result => result?,
	};
	DiffReport::new(arguments, Filesystem::Apfs, report, |info| {
		Volume::ApfsVolume((&info).into())
	})
	.write(arguments.json)
}

fn inputs(arguments: &Arguments) -> (semantic::Input<'_>, semantic::Input<'_>) {
	let options = &arguments.options;
	let encoding = |raw| {
		if raw {
			Encoding::Raw
		} else {
			Encoding::Udif(Default::default())
		}
	};
	(
		semantic::Input {
			path: &arguments.left,
			encoding: encoding(options.left_raw),
			partition: options.left_partition,
		},
		semantic::Input {
			path: &arguments.right,
			encoding: encoding(options.right_raw),
			partition: options.right_partition,
		},
	)
}

fn comparison_policy(arguments: &Arguments) -> (Limits, Options) {
	let mut limits = Limits::default();
	if let Some(max_bytes) = arguments.max_bytes {
		limits.max_bytes = max_bytes;
	}
	let mut options = Options::default();
	options.paths = match arguments.options.paths.unwrap_or_default() {
		PathMode::Exact => PathMatching::ExactStoredText,
		PathMode::Native => PathMatching::Native,
	};
	options.xattrs = arguments.options.xattrs;
	options.finder_info = arguments.options.finder_info;
	#[cfg(feature = "diff-hash")]
	{
		options.contents = match arguments.options.contents.unwrap_or_default() {
			ContentsMode::Exact => Contents::Exact,
			ContentsMode::Blake3 => Contents::Blake3,
			ContentsMode::Blake3Exact => Contents::Blake3Exact,
		};
		options.infer_renames = arguments.options.infer_renames;
	}
	#[cfg(feature = "finder-diff")]
	{
		options.finder_layout = arguments.options.finder_layout;
	}
	(limits, options)
}

#[derive(Serialize)]
struct Input {
	input: InputFormat,
	window: SelectedWindowId,
	#[serde(flatten)]
	volume: Volume,
}

impl Input {
	fn new<I>(input: ComparedInput<I>, raw: bool, volume: &impl Fn(I) -> Volume) -> Self {
		Self {
			input: if raw {
				InputFormat::Raw
			} else {
				InputFormat::Udif
			},
			window: input.window,
			volume: volume(input.identity),
		}
	}
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum InputFormat {
	Raw,
	Udif,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Volume {
	#[cfg(feature = "hfsplus-read")]
	HfsSignature(HfsSignature),
	#[cfg(feature = "apfs-read")]
	ApfsVolume(VolumeIdentity),
}

#[cfg(feature = "hfsplus-read")]
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum HfsSignature {
	HfsPlus,
	HfsX,
}

#[cfg(feature = "hfsplus-read")]
impl From<dmgez_hfsplus::browse::VolumeSignature> for HfsSignature {
	fn from(value: dmgez_hfsplus::browse::VolumeSignature) -> Self {
		use dmgez_hfsplus::browse::VolumeSignature;

		match value {
			VolumeSignature::HfsPlus => Self::HfsPlus,
			VolumeSignature::HfsX => Self::HfsX,
		}
	}
}

#[derive(Serialize)]
struct DiffReport {
	address_space: &'static str,
	filesystem: &'static str,
	left: Input,
	right: Input,
	#[serde(flatten)]
	comparison: Report,
}

impl DiffReport {
	fn new<I>(
		arguments: &Arguments,
		filesystem: Filesystem,
		report: semantic::Report<I>,
		volume: impl Fn(I) -> Volume,
	) -> Self {
		Self {
			address_space: FILESYSTEM_SPACE,
			filesystem: filesystem.as_str(),
			left: Input::new(report.left, arguments.options.left_raw, &volume),
			right: Input::new(report.right, arguments.options.right_raw, &volume),
			comparison: report.comparison,
		}
	}

	fn write(&self, json: bool) -> CliResult<ExitCode> {
		write_report(json, self, |output| self.write_text(output))?;
		Ok(if self.comparison.outcome == Outcome::Equal {
			ExitCode::SUCCESS
		} else {
			ExitCode::from(DIFFERENT_EXIT_CODE)
		})
	}

	fn write_text(&self, output: &mut impl Write) -> io::Result<()> {
		writeln!(output, "address_space = {}", self.address_space)?;
		writeln!(output, "filesystem = {}", self.filesystem)?;
		writeln!(output, "outcome = {}", self.comparison.outcome.as_str())?;
		writeln!(
			output,
			"compared_bytes = {}",
			self.comparison.compared_bytes
		)?;
		write_value(output, "left", &self.left)?;
		write_value(output, "right", &self.right)?;
		write_value(output, "policy", &self.comparison.policy)?;
		write_value(output, "limits", &self.comparison.limits)?;
		for change in &self.comparison.changes {
			write!(output, "{}\t", change.kind.as_str())?;
			write_json(output, &change.path)?;
			if let Some(right_path) = &change.right_path {
				write!(output, "\t{RIGHT_PATH_LABEL} = ")?;
				write_json(output, right_path)?;
			}
			for field in &change.fields {
				write!(output, "\t{}", field.as_str())?;
			}
			if !change.xattrs.is_empty() {
				write!(output, "\txattrs = ")?;
				write_json(output, &change.xattrs)?;
			}
			#[cfg(feature = "finder-diff")]
			if !change.finder_layout.is_empty() {
				write!(output, "\t{} = ", Field::FinderLayout.as_str())?;
				write_json(output, &change.finder_layout)?;
			}
			writeln!(output)?;
		}
		for spellings in &self.comparison.equivalent_path_spellings {
			write_path_pair(
				output,
				EQUIVALENT_PATH_SPELLING_LABEL,
				&spellings.left,
				&spellings.right,
			)?;
		}
		#[cfg(feature = "diff-hash")]
		for candidate in &self.comparison.rename_candidates {
			write_path_pair(
				output,
				RENAME_CANDIDATE_LABEL,
				&candidate.from,
				&candidate.to,
			)?;
		}
		Ok(())
	}
}

fn write_path_pair(
	output: &mut impl Write,
	label: &str,
	left: &[String],
	right: &[String],
) -> io::Result<()> {
	write!(output, "{label}\t")?;
	write_json(output, &left)?;
	write!(output, "\t")?;
	write_json(output, &right)?;
	writeln!(output)
}

fn write_value(output: &mut impl Write, label: &str, value: &impl Serialize) -> io::Result<()> {
	write!(output, "{label} = ")?;
	write_json(output, value)?;
	writeln!(output)
}
