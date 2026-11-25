use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};

/// A shared flag for cooperative cancellation at operation boundaries.
#[derive(Debug, Default, Clone)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
	/// Request termination of an in-progress operation.
	pub fn cancel(&self) {
		self.0.store(true, Ordering::Relaxed);
	}

	/// Check whether termination was requested.
	#[must_use]
	pub fn is_cancelled(&self) -> bool {
		self.0.load(Ordering::Relaxed)
	}

	#[cfg(any(feature = "read", feature = "write"))]
	pub(crate) fn check(&self) -> Result<(), crate::Error> {
		if self.is_cancelled() {
			Err(crate::Error::Cancelled)
		} else {
			Ok(())
		}
	}
}
