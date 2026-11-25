use std::{
	hash::Hash,
	io::{self, Write},
	process::ExitCode,
};

use dmgez_diff::{
	Side,
	semantic::{Limits, Outcome, Report, compare},
};
use dmgez_inspector::SelectedWindowId;
use dmgez_vfs::{
	VolumeId,
	volume::{VolumeBackend, VolumeView},
};
use serde::Serialize;

#[cfg(feature = "hfsplus-read")]
use crate::volume_input::with_hfs_view_with_signature;
use crate::{
	CliResult,
	report_output::{write_json, write_report},
	volume_commands::Filesystem,
};
#[cfg(feature = "apfs-read")]
use crate::{
	volume_commands::{VolumeIdentity, checkpoint, volume_selector, with_named_selector_guidance},
	volume_input::with_apfs_view_for_comparison,
};

use super::{Arguments, DIFFERENT_EXIT_CODE, require_regular_file, with_side};
#[cfg(feature = "apfs-read")]
use super::{LEFT_VOLUME_OPTION, RIGHT_VOLUME_OPTION};

const FILESYSTEM_SPACE: &str = "filesystem";

#[cfg(feature = "hfsplus-read")]
pub(super) fn hfsplus(arguments: &Arguments) -> CliResult<ExitCode> {
	require_regular_inputs(arguments)?;
	let options = &arguments.options;
	let attempt = with_hfs_view_with_signature(
		&arguments.left,
		options.left_raw,
		options.left_partition,
		|left, left_signature| {
			let right = with_hfs_view_with_signature(
				&arguments.right,
				options.right_raw,
				options.right_partition,
				|right, right_signature| {
					Ok(compare_open(
						arguments,
						Filesystem::Hfsplus,
						left,
						right,
						Volume::HfsSignature(left_signature.into()),
						Volume::HfsSignature(right_signature.into()),
					))
				},
			);
			Ok(with_side(right, Side::Right))
		},
	);
	let comparison = with_side(attempt, Side::Left)??;
	comparison?.write(arguments.json)
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
	require_regular_inputs(arguments)?;
	let attempt = with_apfs_view_for_comparison(
		&arguments.left,
		options.left_raw,
		options.left_partition,
		left_volume,
		checkpoint(options.left_xid),
		|left, left_identity| {
			let right = with_apfs_view_for_comparison(
				&arguments.right,
				options.right_raw,
				options.right_partition,
				right_volume,
				checkpoint(options.right_xid),
				|right, right_identity| {
					Ok(compare_open(
						arguments,
						Filesystem::Apfs,
						left,
						right,
						Volume::ApfsVolume(left_identity.into()),
						Volume::ApfsVolume(right_identity.into()),
					))
				},
			)
			.map_err(|error| with_named_selector_guidance(error, RIGHT_VOLUME_OPTION));
			Ok(with_side(right, Side::Right))
		},
	)
	.map_err(|error| with_named_selector_guidance(error, LEFT_VOLUME_OPTION));
	let comparison = with_side(attempt, Side::Left)??;
	comparison?.write(arguments.json)
}

fn require_regular_inputs(arguments: &Arguments) -> CliResult {
	with_side(require_regular_file(&arguments.left), Side::Left)?;
	with_side(require_regular_file(&arguments.right), Side::Right)?;
	Ok(())
}

fn compare_open<L: VolumeBackend, R: VolumeBackend>(
	arguments: &Arguments,
	filesystem: Filesystem,
	left: &mut VolumeView<L>,
	right: &mut VolumeView<R>,
	left_volume: Volume,
	right_volume: Volume,
) -> CliResult<DiffReport>
where
	L::Id: Eq + Hash,
	R::Id: Eq + Hash,
{
	let mut limits = Limits::default();
	if let Some(max_bytes) = arguments.max_bytes {
		limits.max_bytes = max_bytes;
	}
	let left_input = with_side(
		Input::new(left.partition(), arguments.options.left_raw, left_volume),
		Side::Left,
	)?;
	let right_input = with_side(
		Input::new(right.partition(), arguments.options.right_raw, right_volume),
		Side::Right,
	)?;
	Ok(DiffReport {
		address_space: FILESYSTEM_SPACE,
		filesystem: filesystem.as_str(),
		left: left_input,
		right: right_input,
		comparison: compare(left, right, limits, || false)?,
	})
}

#[derive(Serialize)]
struct Input {
	input: InputFormat,
	window: SelectedWindowId,
	#[serde(flatten)]
	volume: Volume,
}

impl Input {
	fn new(window: VolumeId, raw: bool, volume: Volume) -> io::Result<Self> {
		Ok(Self {
			input: if raw {
				InputFormat::Raw
			} else {
				InputFormat::Udif
			},
			window: SelectedWindowId::from_volume(window).ok_or_else(|| {
				io::Error::new(
					io::ErrorKind::Unsupported,
					"unsupported selected disk window",
				)
			})?,
			volume,
		})
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
impl From<dmgez_hfsplus_read::browse::VolumeSignature> for HfsSignature {
	fn from(value: dmgez_hfsplus_read::browse::VolumeSignature) -> Self {
		use dmgez_hfsplus_read::browse::VolumeSignature;

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
			for field in &change.fields {
				write!(output, "\t{}", field.as_str())?;
			}
			writeln!(output)?;
		}
		Ok(())
	}
}

fn write_value(output: &mut impl Write, label: &str, value: &impl Serialize) -> io::Result<()> {
	write!(output, "{label} = ")?;
	write_json(output, value)?;
	writeln!(output)
}
