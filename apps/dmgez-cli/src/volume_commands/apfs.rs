use std::{
	error::Error,
	fmt,
	io::{self, Write},
	num::NonZeroU64,
};

use dmgez_apfs_read::{
	CheckpointCorruption, CheckpointDiagnostic, ObjectId, OpenError, TransactionId, VolumeInfo,
	VolumeRole, VolumeSelector,
};
use dmgez_inspector::{SCHEMA_VERSION, SelectedWindowId};
use serde::{Serialize, Serializer};

use crate::{
	CliResult,
	report_output::{EscapedText, write_report},
	volume_input::apfs_volumes,
};

use super::{
	VOLUME_OPTION, VOLUME_VALUE_NAME, VOLUMES_COMMAND, VolumesArguments, report::write_window,
};

const UUID_SEPARATOR: char = '-';
const UUID_BYTES: usize = 16;
const UUID_GROUP_BYTES: [usize; 5] = [4, 2, 2, 2, 6];
const HEX_DIGITS_PER_BYTE: usize = 2;
const HEX_RADIX: u32 = 16;
const APFS_VOLUME_UUID_KEY: &str = "apfs_volume_uuid";
const APFS_VOLUME_OBJECT_ID_KEY: &str = "apfs_volume_object_id";
const CHECKPOINT_XID_KEY: &str = "checkpoint_xid";

pub(super) fn volume_selector(value: &str) -> io::Result<VolumeSelector> {
	if value.bytes().all(|byte| byte.is_ascii_digit())
		&& let Ok(id) = value.parse::<NonZeroU64>()
	{
		return Ok(VolumeSelector::ObjectId(ObjectId(id.get())));
	}
	if let Some(uuid) = VolumeUuid::parse(value) {
		return Ok(VolumeSelector::Uuid(uuid.0));
	}
	Err(io::Error::new(
		io::ErrorKind::InvalidInput,
		format!("--{VOLUME_OPTION} requires a UUID or a nonzero decimal object ID"),
	))
}

pub(super) fn with_selector_guidance(error: Box<dyn Error>) -> Box<dyn Error> {
	if matches!(
		error.downcast_ref::<OpenError>(),
		Some(OpenError::AmbiguousVolumes(_))
	) {
		Box::new(SelectionRequired(error))
	} else {
		error
	}
}

#[derive(Debug)]
struct SelectionRequired(Box<dyn Error>);

impl fmt::Display for SelectionRequired {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"select an APFS volume with --{VOLUME_OPTION} {VOLUME_VALUE_NAME}"
		)
	}
}

impl Error for SelectionRequired {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		Some(self.0.as_ref())
	}
}

pub(super) fn checkpoint(xid: Option<NonZeroU64>) -> Option<TransactionId> {
	xid.map(|id| TransactionId(id.get()))
}

pub(super) fn volumes(arguments: &VolumesArguments) -> CliResult {
	let input = &arguments.input;
	let discovered = apfs_volumes(
		&input.image,
		input.raw,
		input.partition,
		checkpoint(arguments.xid),
	)?;
	let volume = SelectedWindowId::from_volume(discovered.selected).ok_or_else(|| {
		io::Error::new(io::ErrorKind::Unsupported, "unsupported APFS disk window")
	})?;
	let report = VolumesReport {
		schema_version: SCHEMA_VERSION,
		operation: VOLUMES_COMMAND,
		volume,
		checkpoint_xid: DecimalId(discovered.checkpoint.0),
		volumes: discovered.volumes.iter().map(VolumeEntry::from).collect(),
		checkpoint_diagnostics: discovered
			.diagnostics
			.iter()
			.map(DiagnosticReport::from)
			.collect(),
	};
	write_report(arguments.json, &report, |output| report.write(output))
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct VolumeIdentity {
	uuid: VolumeUuid,
	object_id: DecimalId,
	checkpoint_xid: DecimalId,
}

impl From<&VolumeInfo> for VolumeIdentity {
	fn from(info: &VolumeInfo) -> Self {
		Self {
			uuid: VolumeUuid(info.uuid),
			object_id: DecimalId(info.object_id.0),
			checkpoint_xid: DecimalId(info.checkpoint.0),
		}
	}
}

impl VolumeIdentity {
	pub(super) fn write(self, output: &mut impl Write) -> io::Result<()> {
		writeln!(output, "{APFS_VOLUME_UUID_KEY} = {}", self.uuid)?;
		writeln!(output, "{APFS_VOLUME_OBJECT_ID_KEY} = {}", self.object_id.0)?;
		writeln!(output, "{CHECKPOINT_XID_KEY} = {}", self.checkpoint_xid.0)
	}
}

#[derive(Clone, Copy)]
struct VolumeUuid([u8; UUID_BYTES]);

impl VolumeUuid {
	fn parse(value: &str) -> Option<Self> {
		let mut groups = value.split(UUID_SEPARATOR);
		let mut uuid = [0; UUID_BYTES];
		let mut bytes = uuid.iter_mut();
		for width in UUID_GROUP_BYTES {
			let group = groups.next()?;
			if group.len() != width * HEX_DIGITS_PER_BYTE
				|| !group.bytes().all(|byte| byte.is_ascii_hexdigit())
			{
				return None;
			}
			let (pairs, _) = group.as_bytes().as_chunks::<HEX_DIGITS_PER_BYTE>();
			for pair in pairs {
				let hex = str::from_utf8(pair).ok()?;
				*bytes.next()? = u8::from_str_radix(hex, HEX_RADIX).ok()?;
			}
		}
		groups.next().is_none().then_some(Self(uuid))
	}
}

impl fmt::Display for VolumeUuid {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut bytes = self.0.iter();
		for (index, width) in UUID_GROUP_BYTES.into_iter().enumerate() {
			if index != 0 {
				write!(formatter, "{UUID_SEPARATOR}")?;
			}
			for byte in bytes.by_ref().take(width) {
				write!(formatter, "{byte:02x}")?;
			}
		}
		Ok(())
	}
}

impl Serialize for VolumeUuid {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(self)
	}
}

#[derive(Clone, Copy)]
struct DecimalId(u64);

impl Serialize for DecimalId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.collect_str(&self.0)
	}
}

#[derive(Serialize)]
struct VolumesReport<'volumes> {
	schema_version: u32,
	operation: &'static str,
	volume: SelectedWindowId,
	checkpoint_xid: DecimalId,
	volumes: Vec<VolumeEntry<'volumes>>,
	checkpoint_diagnostics: Vec<DiagnosticReport>,
}

impl VolumesReport<'_> {
	fn write(&self, output: &mut impl Write) -> io::Result<()> {
		write_window(output, self.volume)?;
		writeln!(output, "{CHECKPOINT_XID_KEY} = {}", self.checkpoint_xid.0)?;
		for volume in &self.volumes {
			writeln!(
				output,
				"{}\t{}\t{}\t{}",
				volume.object_id.0,
				volume.uuid,
				volume.role.as_str(),
				EscapedText(volume.name),
			)?;
		}
		for diagnostic in &self.checkpoint_diagnostics {
			diagnostic.write(output)?;
		}
		Ok(())
	}
}

#[derive(Serialize)]
struct VolumeEntry<'volume> {
	uuid: VolumeUuid,
	object_id: DecimalId,
	name: &'volume str,
	role: Role,
}

impl<'volume> From<&'volume VolumeInfo> for VolumeEntry<'volume> {
	fn from(info: &'volume VolumeInfo) -> Self {
		Self {
			uuid: VolumeUuid(info.uuid),
			object_id: DecimalId(info.object_id.0),
			name: info.name.as_str(),
			role: match info.role {
				VolumeRole::Unspecified => Role::Unspecified,
				VolumeRole::Data => Role::Data,
				_ => Role::Unknown,
			},
		}
	}
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Role {
	Unspecified,
	Data,
	Unknown,
}

impl Role {
	const fn as_str(&self) -> &'static str {
		match self {
			Self::Unspecified => "unspecified",
			Self::Data => "data",
			Self::Unknown => "unknown",
		}
	}
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DiagnosticReport {
	Rejected {
		descriptor_block: DecimalId,
		#[serde(skip_serializing_if = "Option::is_none")]
		checkpoint_xid: Option<DecimalId>,
		reason: DiagnosticReason,
	},
	NewerBootstrap {
		advertised_xid: DecimalId,
		selected_xid: DecimalId,
	},
	Unknown,
}

impl From<&CheckpointDiagnostic> for DiagnosticReport {
	fn from(diagnostic: &CheckpointDiagnostic) -> Self {
		match diagnostic {
			CheckpointDiagnostic::Rejected(damage) => Self::Rejected {
				descriptor_block: DecimalId(damage.descriptor.0),
				checkpoint_xid: damage.transaction.map(|id| DecimalId(id.0)),
				reason: match damage.reason {
					CheckpointCorruption::Format(_) => DiagnosticReason::Format,
					CheckpointCorruption::Identity => DiagnosticReason::Identity,
					CheckpointCorruption::Ring => DiagnosticReason::Ring,
					CheckpointCorruption::Object => DiagnosticReason::Object,
					_ => DiagnosticReason::Unknown,
				},
			},
			CheckpointDiagnostic::NewerBootstrap {
				advertised,
				selected,
			} => Self::NewerBootstrap {
				advertised_xid: DecimalId(advertised.0),
				selected_xid: DecimalId(selected.0),
			},
			_ => Self::Unknown,
		}
	}
}

impl DiagnosticReport {
	fn write(&self, output: &mut impl Write) -> io::Result<()> {
		match self {
			Self::Rejected {
				descriptor_block,
				checkpoint_xid,
				reason,
			} => {
				write!(
					output,
					"rejected APFS checkpoint descriptor at block {}",
					descriptor_block.0
				)?;
				if let Some(xid) = checkpoint_xid {
					write!(output, " with XID {}", xid.0)?;
				}
				writeln!(output, " ({})", reason.as_str())
			}
			Self::NewerBootstrap {
				advertised_xid,
				selected_xid,
			} => writeln!(
				output,
				"APFS bootstrap advertised XID {} and the selected checkpoint has XID {}",
				advertised_xid.0, selected_xid.0,
			),
			Self::Unknown => writeln!(output, "unrecognized APFS checkpoint diagnostic"),
		}
	}
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum DiagnosticReason {
	Format,
	Identity,
	Ring,
	Object,
	Unknown,
}

impl DiagnosticReason {
	const fn as_str(&self) -> &'static str {
		match self {
			Self::Format => "format",
			Self::Identity => "identity",
			Self::Ring => "ring",
			Self::Object => "object",
			Self::Unknown => "unknown",
		}
	}
}
