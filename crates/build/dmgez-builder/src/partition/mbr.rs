use dmgez_udif::SECTOR_BYTES;
use part_tables::mbr::build_minimal_mbr;

use super::{Envelope, Error, MBR_HEAD_SECTORS};

const APPLE_HFS_PARTITION_TYPE: u8 = 0xaf;
const UNSPECIFIED_DISK_SIGNATURE: u32 = 0;
// Conventional geometry for legacy CHS fields. The byte range comes from the LBA fields.
const CHS_HEADS: u16 = 255;
const CHS_SECTORS_PER_TRACK: u16 = 63;
const PREFIX_BYTES: usize = (MBR_HEAD_SECTORS * SECTOR_BYTES) as usize;

pub(super) fn generate(volume_bytes: u64) -> Result<Envelope, Error> {
	let sectors = u32::try_from(volume_bytes / SECTOR_BYTES).map_err(|_| Error::Geometry)?;
	let start = MBR_HEAD_SECTORS as u32;
	let sector = build_minimal_mbr(
		UNSPECIFIED_DISK_SIGNATURE,
		&[(APPLE_HFS_PARTITION_TYPE, start, sectors, false)],
		CHS_HEADS,
		CHS_SECTORS_PER_TRACK,
	);
	let mut prefix = vec![0; PREFIX_BYTES];
	prefix[..sector.len()].copy_from_slice(&sector);
	Ok(Envelope {
		prefix,
		suffix: Vec::new(),
	})
}
