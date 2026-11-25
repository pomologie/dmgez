//! Progress reported by build services on the calling thread (C6).

/// An observation about a build in progress. Clients choose how to render it.
/// A completion event follows successful publication or stream completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BuildProgress {
	Discovering,
	SourcesDiscovered {
		entries: usize,
		regular_payload_bytes: u64,
	},
	FinderAssetsPrepared {
		assets: usize,
	},
	WritingImage {
		logical_bytes: u64,
	},
	/// Bytes read from the logical image. Encoding and publication may still be pending.
	ReadingImage {
		logical_bytes_read: u64,
		logical_bytes: u64,
	},
	VerifyingOutput,
	Publishing,
	Completed {
		physical_bytes: u64,
	},
}
