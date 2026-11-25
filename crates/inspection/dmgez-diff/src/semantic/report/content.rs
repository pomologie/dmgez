use serde::Serialize;

use super::{ContentComparison, Policy, Uncompared};

/// Comparison mode for admitted regular-file forks at common stored paths.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Contents {
	/// Compare fork bytes exactly.
	#[default]
	Exact,
	/// Compare the lengths and BLAKE3 digests of the forks.
	Blake3,
	/// Compare lengths and digests, then compare bytes when digests match.
	Blake3Exact,
}

/// How the comparison infers possible renames.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RenameInference {
	/// Match unique BLAKE3 fork fingerprints among unmatched regular paths.
	///
	/// Inference covers unshared files with a nonempty data or resource fork.
	/// A fingerprint includes the lengths and digests of present forks, with
	/// resource-fork absence distinct from a present empty fork.
	UniqueBlake3Forks,
}

/// Possible rename inferred from matching file contents, with both paths
/// retained as ordinary removal and addition changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RenameCandidate {
	/// Removed path, represented by its exact stored components.
	pub from: Vec<String>,
	/// Added path, represented by its exact stored components.
	pub to: Vec<String>,
}

impl RenameCandidate {
	pub(in crate::semantic) fn new(from: &[String], to: &[String]) -> Self {
		Self {
			from: from.to_vec(),
			to: to.to_vec(),
		}
	}
}

impl Policy {
	pub(super) fn with_content_options(mut self, contents: Contents, infer_renames: bool) -> Self {
		self.contents = match (contents, self.contents) {
			(Contents::Exact, comparison) => comparison,
			#[cfg(feature = "finder-layout")]
			(Contents::Blake3, ContentComparison::ExactBytesWithFinderLayout) => {
				ContentComparison::Blake3WithFinderLayout
			}
			#[cfg(feature = "finder-layout")]
			(Contents::Blake3Exact, ContentComparison::ExactBytesWithFinderLayout) => {
				ContentComparison::Blake3ExactWithFinderLayout
			}
			(Contents::Blake3, _) => ContentComparison::Blake3,
			(Contents::Blake3Exact, _) => ContentComparison::Blake3Exact,
		};
		if infer_renames {
			self.rename_inference = Some(RenameInference::UniqueBlake3Forks);
			self.excluded = self
				.excluded
				.strip_suffix(&[Uncompared::RenameInference])
				.expect("the rename inference exclusion ends every baseline policy");
		}
		self
	}
}
