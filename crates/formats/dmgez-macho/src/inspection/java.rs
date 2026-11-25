use super::Region;

pub(super) const JAVA_FIRST_MAJOR_VERSION: u16 = 45;
const JAVA_PREVIEW_FIRST_MAJOR_VERSION: u16 = 56;
const JAVA_PREVIEW_MINOR_VERSION: u16 = u16::MAX;
const JAVA_UTF8_TAG: u8 = 1;
const JAVA_INTEGER_TAG: u8 = 3;
const JAVA_NAME_AND_TYPE_TAG: u8 = 12;
const JAVA_METHOD_HANDLE_TAG: u8 = 15;
const JAVA_PACKAGE_TAG: u8 = 20;

pub(super) fn is_java_class_prefix<E>(
	image: Region,
	read_at: &mut impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<bool, E> {
	// Java ClassFile shares FAT_MAGIC. Only use the Java prefix when the
	// same bytes cannot describe the classic universal file's slice table.
	let Some(
		[
			_,
			_,
			_,
			_,
			minor_hi,
			minor_lo,
			major_hi,
			major_lo,
			pool_hi,
			pool_lo,
			first_tag,
		],
	) = image.read(0, read_at)?
	else {
		return Ok(false);
	};
	let minor = u16::from_be_bytes([minor_hi, minor_lo]);
	let major = u16::from_be_bytes([major_hi, major_lo]);
	let pool_count = u16::from_be_bytes([pool_hi, pool_lo]);
	Ok(major >= JAVA_FIRST_MAJOR_VERSION
		&& (major < JAVA_PREVIEW_FIRST_MAJOR_VERSION
			|| minor == 0
			|| minor == JAVA_PREVIEW_MINOR_VERSION)
		&& pool_count > 1
		// JVM ClassFile constant_pool tags have gaps for reserved values.
		&& matches!(
			first_tag,
			JAVA_UTF8_TAG | JAVA_INTEGER_TAG..=JAVA_NAME_AND_TYPE_TAG | JAVA_METHOD_HANDLE_TAG..=JAVA_PACKAGE_TAG
		))
}
