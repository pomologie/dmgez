//! Choose the scheduling policy for image encoding.

use dmgez_udif::{Error, writer::Execution};

const SERIAL_WORKER_COUNT: usize = 1;
const PARALLEL_FEATURE: &str = "parallel";

/// Select automatic scheduling or honor an explicitly requested worker count.
///
/// # Errors
/// Returns an unavailable-feature error if multiple workers were requested
/// from a serial-only runtime.
pub fn scheduled_execution(workers: Option<usize>) -> Result<Execution, Error> {
	if cfg!(feature = "parallel") {
		return Ok(Execution::Parallel {
			workers: workers.unwrap_or(Execution::AUTO_WORKERS),
		});
	}
	match workers {
		None | Some(Execution::AUTO_WORKERS | SERIAL_WORKER_COUNT) => Ok(Execution::Serial),
		Some(_) => Err(Error::FeatureNotCompiled(PARALLEL_FEATURE)),
	}
}
