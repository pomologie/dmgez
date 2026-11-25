use std::collections::BTreeMap;

use dmgez_config::{Capacity, CompressionCache, IconSlot, LocalPath, VolumeIcon};
use dmgez_udif::writer::{Cancellation, WriteOptions};

use super::{FilesystemProfile, FinderPlan, OutputPlan, PreflightError, ReleasePlan, Resource};

/// Effective per-build working limits. Byte limits specify owned allocations,
/// not a process RSS guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ResourceLimits {
	pub memory_bytes: usize,
	pub max_metadata_bytes: usize,
	pub max_entries: usize,
	/// Worst-case data run count permitted before streaming begins.
	pub max_runs: usize,
}

/// An admitted configuration ready for source discovery and geometry planning.
#[derive(Clone, Debug)]
pub struct ExecutionProfile {
	pub(super) filesystem: FilesystemProfile,
	pub(super) capacity: Capacity,
	pub(super) free_space_bytes: u64,
	pub(super) output: OutputPlan,
	pub(super) verify_output: bool,
	pub(super) release: ReleasePlan,
	pub(super) cache: Option<CompressionCache>,
	pub(super) finder: FinderPlan,
	pub(super) volume_icon: Option<VolumeIcon>,
	pub(super) limits: ResourceLimits,
	pub(super) reproducible: bool,
	pub(super) source_date_epoch: Option<u64>,
	pub(super) fail_on_metadata_loss: bool,
	pub(super) writer: WriteOptions,
}

impl ExecutionProfile {
	#[must_use]
	pub const fn filesystem(&self) -> FilesystemProfile {
		self.filesystem
	}

	#[must_use]
	pub const fn capacity(&self) -> Capacity {
		self.capacity
	}

	#[must_use]
	pub const fn free_space_bytes(&self) -> u64 {
		self.free_space_bytes
	}

	#[must_use]
	pub const fn output(&self) -> OutputPlan {
		self.output
	}

	#[must_use]
	pub const fn verify_output(&self) -> bool {
		self.verify_output
	}

	#[must_use]
	pub const fn release(&self) -> ReleasePlan {
		self.release
	}

	/// Returns the cache settings admitted for an unsigned UDZO build.
	#[must_use]
	pub const fn cache(&self) -> Option<&CompressionCache> {
		self.cache.as_ref()
	}

	#[must_use]
	pub const fn finder(&self) -> FinderPlan {
		self.finder
	}

	/// Returns the admitted prebuilt icon asset path, relative to the project if applicable.
	#[must_use]
	pub fn volume_icon_source(&self) -> Option<&LocalPath> {
		match self.volume_icon.as_ref() {
			Some(VolumeIcon::Source(source)) => Some(source),
			_ => None,
		}
	}

	/// Returns admitted PNG paths by point-size and scale slot.
	#[must_use]
	pub fn volume_icon_images(&self) -> Option<&BTreeMap<IconSlot, LocalPath>> {
		match self.volume_icon.as_ref() {
			Some(VolumeIcon::Images(images)) => Some(images),
			_ => None,
		}
	}

	/// Returns the admitted badge artwork path, relative to the project if applicable.
	#[must_use]
	pub fn volume_icon_badge(&self) -> Option<&LocalPath> {
		match self.volume_icon.as_ref() {
			Some(VolumeIcon::Badge(badge)) => Some(badge),
			_ => None,
		}
	}

	#[must_use]
	pub const fn limits(&self) -> ResourceLimits {
		self.limits
	}

	#[must_use]
	pub const fn reproducible(&self) -> bool {
		self.reproducible
	}

	#[must_use]
	pub const fn source_date_epoch(&self) -> Option<u64> {
		self.source_date_epoch
	}

	#[must_use]
	pub const fn fail_on_metadata_loss(&self) -> bool {
		self.fail_on_metadata_loss
	}

	#[must_use]
	pub const fn write_options(&self) -> &WriteOptions {
		&self.writer
	}

	pub(crate) fn set_cancellation(&mut self, cancellation: Cancellation) {
		self.writer.cancellation = cancellation;
	}

	/// Checks the logical size selected by the filesystem planner before starting a write.
	///
	/// # Errors
	///
	/// Rejects a size violating the configured capacity, UDIF geometry, or
	/// the configured worst-case run budget.
	pub fn validate_logical_bytes(&self, logical_bytes: u64) -> Result<(), PreflightError> {
		match self.capacity {
			Capacity::Exact(bytes) if logical_bytes != bytes.get() => {
				return Err(PreflightError::Capacity);
			}
			Capacity::ShrinkWithin(bytes) if logical_bytes > bytes.get() => {
				return Err(PreflightError::Capacity);
			}
			_ => {}
		}
		if self.writer.max_data_runs(logical_bytes)? > self.limits.max_runs {
			return Err(PreflightError::ResourceLimit(Resource::Runs));
		}
		Ok(())
	}
}
