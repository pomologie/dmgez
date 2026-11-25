use std::{num::NonZeroU64, ops::Range};

use dmgez_udif::{
	Error as SourceError,
	reader::{Image, ReadAt},
};

use crate::{Error, Filesystem, Layout, Limits, Partition, PartitionId, Window, discovery};

/// Explicit selection of an admitted whole-disk volume or a partition slot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum VolumeId {
	WholeDisk,
	Partition(PartitionId),
}

/// Byte-addressed logical storage with explicit cache mutability.
///
/// Implementors must preserve the admitted length and reject changed sources,
/// out-of-bounds requests and short reads. Empty reads at EOF are legal and
/// must still validate source identity.
pub trait LogicalDisk {
	fn len(&self) -> u64;
	fn is_empty(&self) -> bool {
		self.len() == 0
	}
	/// Returns source, bounds, integrity, and codec errors without zero filling.
	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError>;
}

impl<S: LogicalDisk + ?Sized> LogicalDisk for &mut S {
	fn len(&self) -> u64 {
		(**self).len()
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		(**self).read_exact_at(offset, output)
	}
}

impl<S: ReadAt> LogicalDisk for Image<S> {
	fn len(&self) -> u64 {
		self.descriptor().logical_bytes
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		Image::read_exact_at(self, offset, output)
	}
}

/// Explicitly treats a finite source as a raw logical disk.
#[derive(Debug)]
pub struct Raw<S>(pub S);

impl<S: ReadAt> LogicalDisk for Raw<S> {
	fn len(&self) -> u64 {
		self.0.len()
	}

	fn read_exact_at(&mut self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
		self.0.check_unchanged()?;
		self.0.read_exact_at(offset, output)?;
		self.0.check_unchanged()
	}
}

/// Discovered layout scoped to its admitted logical source.
#[derive(Debug)]
pub struct Disk<S> {
	source: S,
	layout: Layout,
	probe_bytes_remaining: u64,
}

impl<S: LogicalDisk> Disk<S> {
	/// Reads interpretation-relevant map bytes under the configured limits.
	///
	/// # Errors
	/// Rejects unsupported, conflicting, or malformed map metadata and failed reads.
	pub fn discover(mut source: S, limits: Limits) -> Result<Self, Error> {
		let (layout, probe_bytes_remaining) = discovery::discover(&mut source, limits)?;
		Ok(Self {
			source,
			layout,
			probe_bytes_remaining,
		})
	}

	/// Recognize a partitionless filesystem using a caller-owned geometry probe.
	///
	/// The probe runs only when discovery found an unknown layout. It receives a
	/// read-only window over the logical source and supplies the byte length of
	/// the validated extent starting at zero, or `None` for unrecognized data.
	/// The caller is responsible for the probe's resource limits.
	///
	/// # Errors
	/// Propagates probe failures and rejects extents outside the logical source.
	/// The layout stays unchanged on error.
	pub fn recognize_partitionless<E: From<Error>>(
		&mut self,
		filesystem: Filesystem,
		probe: impl FnOnce(Window<'_, S>) -> Result<Option<NonZeroU64>, E>,
	) -> Result<(), E> {
		if !matches!(self.layout, Layout::Unknown) {
			return Ok(());
		}
		let available = self.source.len();
		let Some(len) = probe(Window::new(&mut self.source, 0..available))? else {
			return Ok(());
		};
		let len = len.get();
		if len > available {
			return Err(Error::OutOfRange {
				offset: 0,
				len,
				available,
			}
			.into());
		}
		self.layout = Layout::Partitionless {
			filesystem,
			range: 0..len,
		};
		Ok(())
	}

	#[must_use]
	pub const fn layout(&self) -> &Layout {
		&self.layout
	}

	/// Resolve a requested filesystem data extent, or the only eligible extent.
	///
	/// # Errors
	/// Rejects missing selectors, metadata-only entries, and automatic selection
	/// with zero or multiple eligible extents.
	pub fn select_volume(&self, requested: Option<VolumeId>) -> Result<VolumeId, Error> {
		if let Some(id) = requested {
			self.volume_data_range(id)?;
			return Ok(id);
		}
		match &self.layout {
			Layout::Partitionless { .. } => Ok(VolumeId::WholeDisk),
			Layout::Partitioned { partitions, .. } => {
				let mut eligible = partitions.iter().filter(|entry| entry.data_range.is_some());
				let first = eligible.next().ok_or(Error::NoVolume)?;
				let candidates = 1 + eligible.count();
				if candidates != 1 {
					return Err(Error::AmbiguousVolumes { candidates });
				}
				Ok(VolumeId::Partition(first.id))
			}
			Layout::Unknown => Err(Error::NoVolume),
		}
	}

	/// Return a window over the complete partition or admitted partitionless volume.
	///
	/// # Errors
	/// Fails if the requested record does not exist in this disk view.
	pub fn window(&mut self, id: VolumeId) -> Result<Window<'_, S>, Error> {
		let range = match &self.layout {
			Layout::Partitionless { range, .. } if id == VolumeId::WholeDisk => range.clone(),
			_ => self.partition(id)?.range.clone(),
		};
		Ok(Window::new(&mut self.source, range))
	}

	/// Return the admitted filesystem data extent inside the selected record.
	///
	/// # Errors
	/// Fails if the requested record is missing or has no filesystem data window.
	pub fn data_window(&mut self, id: VolumeId) -> Result<Window<'_, S>, Error> {
		let range = self.volume_data_range(id)?.clone();
		Ok(Window::new(&mut self.source, range))
	}

	/// Recognize an HFS+ or HFSX signature and geometry inside an admitted data extent.
	/// The returned byte length is relative to the start of the extent. Header reads
	/// share the probe budget established by [`Self::discover`].
	///
	/// # Errors
	/// Rejects a missing extent, exhausted probe budget, failed reads, and invalid geometry.
	pub fn probe_hfs_geometry(&mut self, id: VolumeId) -> Result<Option<(Filesystem, u64)>, Error> {
		let range = self.volume_data_range(id)?.clone();
		let mut window = Window::new(&mut self.source, range);
		discovery::hfs_geometry(window.len(), |offset, output| {
			self.probe_bytes_remaining = self
				.probe_bytes_remaining
				.checked_sub(output.len() as u64)
				.ok_or(discovery::PROBE_LIMIT)?;
			window.read_exact_at(offset, output)
		})
	}

	fn volume_data_range(&self, id: VolumeId) -> Result<&Range<u64>, Error> {
		match &self.layout {
			Layout::Partitionless { range, .. } if id == VolumeId::WholeDisk => Ok(range),
			_ => {
				let partition = self.partition(id)?;
				partition
					.data_range
					.as_ref()
					.ok_or(Error::NoDataWindow(partition.id))
			}
		}
	}

	fn partition(&self, id: VolumeId) -> Result<&Partition, Error> {
		if let (Layout::Partitioned { partitions, .. }, VolumeId::Partition(id)) =
			(&self.layout, id)
			&& let Some(partition) = partitions.iter().find(|entry| entry.id == id)
		{
			return Ok(partition);
		}
		Err(Error::UnknownVolume(id))
	}
}
