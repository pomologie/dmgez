use std::{borrow::Cow, io};

use dmgez_builder::manifest::SourceSnapshot;
use dmgez_core::source::CapturedFile;
use hfsplus_core::{ByteLen, ByteOffset, ReadAt};

#[derive(Debug)]
pub(super) enum Payload<'plan> {
	File(&'plan SourceSnapshot),
	Bytes(Cow<'plan, [u8]>),
}

impl ReadAt for Payload<'_> {
	type Error = io::Error;

	fn len(&self) -> ByteLen {
		match self {
			Self::File(source) => ByteLen(source.metadata().size),
			Self::Bytes(data) => ByteLen::from(data.len()),
		}
	}

	fn read_exact_at(&self, offset: ByteOffset, output: &mut [u8]) -> io::Result<()> {
		match self {
			Self::File(source) => read_host_source(source, offset, output),
			Self::Bytes(data) => data
				.as_ref()
				.read_exact_at(offset, output)
				.map_err(io::Error::other),
		}
	}
}

pub(crate) fn read_host_source(
	source: &SourceSnapshot,
	offset: ByteOffset,
	output: &mut [u8],
) -> io::Result<()> {
	let file = source.open_regular().map_err(io::Error::other)?;
	let captured = CapturedFile::new(file).map_err(io::Error::other)?;
	captured
		.read_exact_at(offset.0, output)
		.map_err(io::Error::other)?;
	source.validate().map_err(io::Error::other)
}
