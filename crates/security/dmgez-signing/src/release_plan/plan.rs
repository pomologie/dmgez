use crate::ResolvedBackend;

use super::{ReleaseApplications, ReleaseError, ReleaseReport, ReleaseRun, ReleaseStage, Step};

const REPORT_VERSION: u16 = 1;

/// Requirements resolved before any stage invokes tools or reads credentials.
#[derive(Clone, Copy, Debug)]
pub struct ReleasePlan {
	pub applications: ReleaseApplications,
	pub sign_image: Option<ResolvedBackend>,
	pub notarize: Option<ResolvedBackend>,
	pub staple: Option<ResolvedBackend>,
	pub assess_native_policy: bool,
}

impl ReleasePlan {
	/// Starts an explicit release run with fixed backend and stage choices.
	///
	/// # Errors
	/// Returns an error when notarization or native policy requires a signed
	/// image, or when the plan requests a ticket without notarization.
	pub fn start(self) -> Result<ReleaseRun, ReleaseError> {
		if self.staple.is_some() && self.notarize.is_none() {
			return Err(ReleaseError::NotarizationRequired);
		}
		if (self.notarize.is_some() || self.assess_native_policy) && self.sign_image.is_none() {
			return Err(ReleaseError::SignedImageRequired);
		}
		let applications = match self.applications {
			ReleaseApplications::Preserve => Step::skipped(ReleaseStage::Applications),
			ReleaseApplications::Verify(backend) | ReleaseApplications::Sign(backend) => {
				Step::required(ReleaseStage::Applications, Some(backend))
			}
		};
		let assess = self.assess_native_policy.then_some(ResolvedBackend::Native);
		let verify_unsigned = if self.sign_image.is_some() {
			Step::required(ReleaseStage::VerifyUnsignedImage, None)
		} else {
			Step::skipped(ReleaseStage::VerifyUnsignedImage)
		};
		Ok(ReleaseRun {
			applications: self.applications,
			report: ReleaseReport {
				schema_version: REPORT_VERSION,
				steps: [
					applications,
					Step::required(ReleaseStage::BuildUnsigned, None),
					verify_unsigned,
					Step::with_backend(ReleaseStage::SignImage, self.sign_image),
					Step::with_backend(ReleaseStage::Notarize, self.notarize),
					Step::with_backend(ReleaseStage::Staple, self.staple),
					Step::required(ReleaseStage::VerifyImage, None),
					Step::with_backend(ReleaseStage::AssessNativePolicy, assess),
					Step::required(ReleaseStage::Publish, None),
				],
				unsigned: None,
				artifact: None,
				notarization: None,
			},
		})
	}
}
