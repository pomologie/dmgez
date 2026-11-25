#[cfg(feature = "read")]
use crate::Error;

#[cfg(all(test, feature = "read"))]
#[path = "../verification/binary_geometry.rs"]
mod geometry_properties;

#[cfg(feature = "read")]
pub(crate) fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], Error> {
	bytes
		.get(offset..)
		.and_then(|remaining| remaining.first_chunk())
		.copied()
		.ok_or(Error::Corrupt("truncated fixed-width field"))
}

#[cfg(feature = "read")]
pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Error> {
	field(bytes, offset).map(u32::from_be_bytes)
}

#[cfg(feature = "read")]
pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Error> {
	field(bytes, offset).map(u64::from_be_bytes)
}

#[cfg(feature = "write")]
pub(crate) fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
	bytes[offset..][..size_of::<u32>()].copy_from_slice(&value.to_be_bytes());
}

#[cfg(feature = "write")]
pub(crate) fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
	bytes[offset..][..size_of::<u64>()].copy_from_slice(&value.to_be_bytes());
}
