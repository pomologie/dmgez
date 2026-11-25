use crate::trailer::{PARTITION_IMAGE, SEGMENT_ID_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputMetadata {
	pub(crate) variant: u32,
	pub(crate) segment_id: [u8; SEGMENT_ID_BYTES],
	pub(crate) include_partition_list: bool,
}

impl Default for OutputMetadata {
	fn default() -> Self {
		Self {
			variant: PARTITION_IMAGE,
			segment_id: [0; SEGMENT_ID_BYTES],
			include_partition_list: false,
		}
	}
}

#[cfg(feature = "read")]
pub use rebuild::{RebuildMetadata, write_rebuilt_image};

#[cfg(feature = "read")]
mod rebuild {
	use std::io::{Read, Write};

	use crate::Error;

	use super::OutputMetadata;
	use crate::writer::{Profile, WriteOptions, WriteReport, write_with_metadata};

	/// Checked container metadata and geometry preserved by a rebuild.
	#[derive(Debug, Clone, Copy)]
	pub struct RebuildMetadata {
		pub(crate) metadata: OutputMetadata,
		pub(crate) logical_bytes: u64,
		pub(crate) source_profile: Profile,
	}

	impl RebuildMetadata {
		/// The input codec profile evidenced by its runs and buffer field.
		#[must_use]
		pub const fn source_profile(&self) -> Profile {
			self.source_profile
		}

		/// Set the size of an edited logical disk while retaining its admitted
		/// container fields. The writer validates the new size before encoding.
		#[must_use]
		pub const fn with_logical_bytes(mut self, logical_bytes: u64) -> Self {
			self.logical_bytes = logical_bytes;
			self
		}

		/// Compare the fields that must survive re-encoding. Codec and physical
		/// block-map layout are deliberately excluded.
		#[must_use]
		pub fn matches_preserved_fields(&self, other: &Self) -> bool {
			self.metadata == other.metadata && self.logical_bytes == other.logical_bytes
		}
	}

	/// Encode a finite logical disk at the admitted size and with the retained
	/// container metadata. A conversion passes the original descriptor unchanged.
	/// # Errors
	/// Rejects unsupported target codecs, invalid lengths and failed I/O.
	pub fn write_rebuilt_image(
		source: impl Read,
		sink: impl Write,
		options: &WriteOptions,
		metadata: RebuildMetadata,
	) -> Result<WriteReport, Error> {
		if !matches!(options.profile, Profile::Udro | Profile::Udzo) {
			return Err(Error::Unsupported("rebuild target codec"));
		}
		write_with_metadata(
			source,
			metadata.logical_bytes,
			sink,
			options,
			metadata.metadata,
		)
	}
}
