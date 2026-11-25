//! Admission rules for partitioned project output.

use dmgez_config::{ImageFormat, Partition};

use super::{BuildFeature, PreflightError};
use crate::partition::Layout;

pub(super) fn partition_layout(
	partition: Partition,
	format: ImageFormat,
) -> Result<Layout, PreflightError> {
	let layout = match partition {
		Partition::None => return Ok(Layout::Partitionless),
		Partition::Apm => Layout::Apm,
		Partition::Gpt => Layout::Gpt,
		Partition::Mbr => Layout::Mbr,
		_ => return Err(PreflightError::Partition(partition)),
	};
	if !cfg!(feature = "partition-write") {
		return Err(PreflightError::FeatureNotCompiled(
			BuildFeature::PartitionWrite,
		));
	}
	if !matches!(
		format,
		ImageFormat::Udro | ImageFormat::Udzo | ImageFormat::Ulfo | ImageFormat::Ulmo
	) {
		return Err(PreflightError::PartitionFormat { partition, format });
	}
	Ok(layout)
}
