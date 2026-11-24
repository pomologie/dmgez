use crate::{Error, blkx::Kind};

use super::{Image, ReadAt, range_len};

impl<S: ReadAt> Image<S> {
	pub(super) fn read_compressed(
		&mut self,
		index: usize,
		within: u64,
		output: &mut [u8],
	) -> Result<(), Error> {
		if self
			.cache
			.as_ref()
			.is_none_or(|(cached, _)| *cached != index)
		{
			self.cache = None;
			let run = &self.runs[index];
			let decode = decoder(run.kind)?;
			let offset = run.physical.start;
			let logical_offset = run.logical.start;
			let mut stored = vec![0; range_len(&run.physical) as usize];
			self.source.read_exact_at(offset, &mut stored)?;
			let decoded =
				decode(&stored, range_len(&run.logical) as usize).ok_or(match run.kind {
					Kind::Lzfse => Error::InvalidLzfse {
						offset,
						logical_offset,
					},
					Kind::Lzma2 => Error::InvalidLzma {
						offset,
						logical_offset,
					},
					Kind::Zlib => Error::InvalidZlib {
						offset,
						logical_offset,
					},
					_ => Error::Corrupt("uncompressed run entered decoder"),
				})?;
			self.cache = Some((index, decoded));
		}
		let (_, decoded) = self
			.cache
			.as_ref()
			.ok_or(Error::Corrupt("missing decoded run"))?;
		let within = within as usize;
		output.copy_from_slice(&decoded[within..within + output.len()]);
		Ok(())
	}
}

type Decoder = fn(&[u8], usize) -> Option<Vec<u8>>;

fn decoder(kind: Kind) -> Result<Decoder, Error> {
	match kind {
		#[cfg(feature = "zlib")]
		Kind::Zlib => Ok(crate::codec::inflate),
		#[cfg(not(feature = "zlib"))]
		Kind::Zlib => Err(Error::FeatureNotCompiled("zlib")),
		#[cfg(feature = "lzfse")]
		Kind::Lzfse => Ok(crate::lzfse::decode),
		#[cfg(not(feature = "lzfse"))]
		Kind::Lzfse => Err(Error::FeatureNotCompiled("lzfse")),
		#[cfg(feature = "lzma")]
		Kind::Lzma2 => Ok(crate::lzma::decode),
		#[cfg(not(feature = "lzma"))]
		Kind::Lzma2 => Err(Error::FeatureNotCompiled("lzma")),
		_ => Err(Error::Corrupt("uncompressed run entered decoder")),
	}
}
