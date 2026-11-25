use crate::blkx::Kind;

use super::{Image, ReadAt, RunDescriptor, RunEncoding};

impl<S: ReadAt> Image<S> {
	/// Inspect the normalized map in logical order without reading any data fork payload.
	pub fn runs(&self) -> impl ExactSizeIterator<Item = RunDescriptor> + '_ {
		self.logical_order
			.iter()
			.map(|&index| self.describe_run(index))
	}

	/// Returns retained metadata for a zero-based ordinal in logical order.
	///
	/// Returns `None` beyond the end of the index. No payload bytes are read.
	#[must_use]
	pub fn run_at(&self, ordinal: usize) -> Option<RunDescriptor> {
		let index = *self.logical_order.get(ordinal)?;
		Some(self.describe_run(index))
	}

	fn describe_run(&self, index: usize) -> RunDescriptor {
		let run = &self.runs[index];
		let encoding = match run.kind {
			Kind::Raw => RunEncoding::Raw,
			Kind::Zero => RunEncoding::Zero,
			Kind::Ignore => RunEncoding::Ignore,
			Kind::Zlib => RunEncoding::Zlib,
			Kind::Lzfse => RunEncoding::Lzfse,
			Kind::Lzma2 => RunEncoding::Lzma2,
			Kind::Unsupported(kind) => RunEncoding::Unsupported(kind),
			Kind::Comment | Kind::End => unreachable!("index contains only logical runs"),
		};
		RunDescriptor {
			logical: run.logical.clone(),
			physical: run.physical.clone(),
			encoding,
		}
	}
}
