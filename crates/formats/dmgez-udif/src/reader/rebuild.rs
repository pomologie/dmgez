use crate::{
	Error, blkx, resource, trailer,
	writer::{OutputMetadata, Profile, RebuildMetadata},
};

use super::{Image, ReadAt, RunEncoding};

const RAW_ONLY_BUFFER_SECTORS: u32 = 0;

impl<S: ReadAt> Image<S> {
	/// Admit the geometry and original metadata for a lossless container rebuild.
	/// Run placement, compression, XML serialization and checksums are regenerated.
	/// # Errors
	/// Refuses opaque resources or payload gaps, unknown run types, unqualified
	/// maps and checksum descriptors, or an unavailable source.
	pub fn rebuild_metadata(&self) -> Result<RebuildMetadata, Error> {
		let desc = self.descriptor();
		let (map, include_partition_list) = resource::rebuild_table(&desc.resources)?;
		let [table] = desc.tables.as_slice() else {
			return Err(Error::Unsupported(
				"conversion requires one whole-disk block map",
			));
		};
		let trailer_start = desc.physical_bytes - trailer::BYTES as u64;
		if desc.data_fork.start != 0
			|| desc.data_fork.end != desc.resource_xml.start
			|| desc.resource_xml.end != trailer_start
			|| table.logical != (0..desc.logical_bytes)
			|| table.records != self.runs().len() + 1
			|| !blkx::has_rebuild_layout(map, desc.data_fork.end)?
		{
			return Err(Error::Unsupported(
				"opaque container layout cannot be preserved by conversion",
			));
		}
		let source_profile = if table.buffer_sectors == RAW_ONLY_BUFFER_SECTORS {
			Profile::Udro
		} else {
			Profile::Udzo
		};
		let mut physical_end = 0;
		for run in self.runs() {
			match (source_profile, run.encoding) {
				(_, RunEncoding::Raw) | (Profile::Udzo, RunEncoding::Zlib | RunEncoding::Zero) => {}
				_ => return Err(Error::Unsupported("unqualified conversion source run")),
			}
			if run.physical.start != physical_end {
				return Err(Error::Unsupported(
					"unmapped data-fork bytes cannot be preserved by conversion",
				));
			}
			physical_end = run.physical.end;
		}
		if physical_end != desc.data_fork.end {
			return Err(Error::Unsupported(
				"unmapped data-fork bytes cannot be preserved by conversion",
			));
		}
		let mut trailer_bytes = [0; trailer::BYTES];
		self.source.check_unchanged()?;
		self.source
			.read_exact_at(trailer_start, &mut trailer_bytes)?;
		self.source.check_unchanged()?;
		if !trailer::has_plain_checksums(&trailer_bytes) {
			return Err(Error::Unsupported(
				"opaque checksum metadata cannot be preserved by conversion",
			));
		}
		Ok(RebuildMetadata {
			metadata: OutputMetadata {
				variant: desc.variant,
				segment_id: desc.segment_id,
				include_partition_list,
			},
			logical_bytes: desc.logical_bytes,
			source_profile,
		})
	}
}
