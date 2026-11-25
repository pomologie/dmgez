use crate::{Error, SECTOR_BYTES, blkx::Kind};

use super::{Profile, RawChunk, StoredChunk, WriteOptions};

impl RawChunk {
	pub(super) fn encode(
		self,
		options: &WriteOptions,
		#[cfg(feature = "write-cache")] cache: Option<&super::cache::ActiveCache<'_>>,
	) -> Result<StoredChunk, Error> {
		options.cancellation.check()?;
		let sectors = self.0.len() as u64 / SECTOR_BYTES;
		if options.profile != Profile::Udro && self.0.iter().all(|&byte| byte == 0) {
			return Ok(StoredChunk {
				sectors,
				kind: Kind::Zero,
				bytes: Vec::new(),
			});
		}
		let compressed = match options.profile {
			#[cfg(feature = "zlib")]
			Profile::Udzo => {
				#[cfg(feature = "write-cache")]
				let encoded = super::cache::deflate(&self.0, options.zlib_level, cache)?;
				#[cfg(not(feature = "write-cache"))]
				let encoded = crate::codec::deflate(&self.0, options.zlib_level)?;
				encoded.map(|bytes| (Kind::Zlib, bytes))
			}
			#[cfg(feature = "lzfse")]
			Profile::Ulfo => crate::lzfse::encode(&self.0)?.map(|bytes| (Kind::Lzfse, bytes)),
			#[cfg(feature = "lzma")]
			Profile::Ulmo => crate::lzma::encode(&self.0)?.map(|bytes| (Kind::Lzma2, bytes)),
			_ => None,
		};
		let (kind, bytes) = compressed.unwrap_or((Kind::Raw, self.0));
		Ok(StoredChunk {
			sectors,
			kind,
			bytes,
		})
	}
}
