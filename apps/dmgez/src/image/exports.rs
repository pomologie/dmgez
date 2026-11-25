use std::{path::Path, sync::Arc};

use dmgez_inspector::{InspectionReport, RunPage, VerificationReport};
use dmgez_runtime::image::{InspectionExportError, MAX_RUN_EXPORT_BYTES, ReportExport, export_run};
use dmgez_udif::Cancellation;

use super::{ImageSnapshot, ImageTab};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
	Inspection,
	Verification,
	Run(usize),
}

enum Content {
	Inspection(Box<InspectionReport>),
	Verification(Arc<VerificationReport>),
	Run(usize),
}

pub(crate) struct Export {
	snapshot: Arc<ImageSnapshot>,
	content: Content,
}

impl ImageTab {
	pub(crate) fn export(&self, kind: Kind) -> Option<Export> {
		let content = match kind {
			Kind::Inspection => Content::Inspection(Box::new(self.report.clone())),
			Kind::Verification => Content::Verification(Arc::clone(self.verification.as_ref()?)),
			Kind::Run(ordinal) => {
				let selected =
					selected_run(self.report.run_page.as_ref()?, self.run_navigation.selected)?;
				if selected != ordinal {
					return None;
				}
				Content::Run(ordinal)
			}
		};
		Some(Export {
			snapshot: Arc::clone(&self.snapshot),
			content,
		})
	}
}

impl Export {
	pub(crate) fn execute(
		self,
		output: &Path,
		cancellation: &Cancellation,
	) -> Result<(), InspectionExportError> {
		match self.content {
			Content::Inspection(report) => self.snapshot.image.export_json(
				ReportExport::Inspection(&report),
				output,
				cancellation,
			),
			Content::Verification(report) => self.snapshot.image.export_json(
				ReportExport::Verification(&report),
				output,
				cancellation,
			),
			Content::Run(ordinal) => {
				export_run(&self.snapshot.source.0, ordinal, output, cancellation)
			}
		}
	}
}

pub(crate) fn selected_run(page: &RunPage, ordinal: Option<usize>) -> Option<usize> {
	let ordinal = ordinal?;
	let start = usize::try_from(page.offset).ok()?;
	let run = page.runs.get(ordinal.checked_sub(start)?)?;
	(run.decoder_available && run.logical.length_bytes <= MAX_RUN_EXPORT_BYTES).then_some(ordinal)
}
