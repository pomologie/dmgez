use dmgez_udif::SECTOR_BYTES;
use part_tables::apm::build_minimal_apm;

use super::{APM_HEAD_SECTORS, Envelope, Error, VOLUME_PARTITION_NAME};

const HFS_PARTITION_TYPE: &str = "Apple_HFS";

pub(super) fn generate(volume_bytes: u64, disk_bytes: u64) -> Result<Envelope, Error> {
	let volume_blocks = u32::try_from(volume_bytes / SECTOR_BYTES).map_err(|_| Error::Geometry)?;
	let disk_blocks = u32::try_from(disk_bytes / SECTOR_BYTES).map_err(|_| Error::Geometry)?;
	let head_blocks = u32::try_from(APM_HEAD_SECTORS).map_err(|_| Error::Geometry)?;
	let block_bytes = u32::try_from(SECTOR_BYTES).map_err(|_| Error::Geometry)?;
	let mut apm = build_minimal_apm(
		&[(HFS_PARTITION_TYPE.to_owned(), head_blocks, volume_blocks)],
		block_bytes,
		disk_blocks,
	);
	let [map, volume] = apm.entries.as_mut_slice() else {
		return Err(Error::Geometry);
	};
	map.block_count = head_blocks - 1;
	map.data_count = map.block_count;
	volume.name = VOLUME_PARTITION_NAME.to_owned();
	let mut prefix = apm.build_apm_blocks(None);
	let prefix_bytes = (APM_HEAD_SECTORS * SECTOR_BYTES) as usize;
	if prefix.len() > prefix_bytes {
		return Err(Error::Geometry);
	}
	prefix.resize(prefix_bytes, 0);
	Ok(Envelope {
		prefix,
		suffix: Vec::new(),
	})
}
