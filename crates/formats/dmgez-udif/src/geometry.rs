/// UDIF logical sector size in bytes, independent of filesystem block size.
pub const SECTOR_BYTES: u64 = 512;

/// Decoder output space needed to distinguish an exact stream from an overrun.
#[cfg(feature = "read")]
pub(crate) const DECODER_SPARE_BYTES: usize = 1;
