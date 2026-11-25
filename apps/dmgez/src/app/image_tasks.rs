use dmgez_inspector::{RunPage, VerificationReport};
use dmgez_udif::{Cancellation, Error};
use eframe::egui::Context;

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use crate::image::browse::{self, Observation, history::Direction};
use crate::inspector::Action;

use super::{App, Completion, Document, Opened};

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
mod folder;

pub(super) enum ImageCompletion {
	Runs(Result<(RunPage, Option<usize>), Error>),
	Checked(Box<VerificationReport>),
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	Browsed {
		direction: Option<Direction>,
		result: Result<Observation, browse::Error>,
	},
	Exported(Result<std::path::PathBuf, dmgez_runtime::image::InspectionExportError>),
}

impl App {
	pub(super) fn image_action(&mut self, ctx: &Context, action: Action) {
		let Some(Opened {
			document: Document::Image(image),
			..
		}) = &mut self.opened
		else {
			return;
		};
		let snapshot = image.snapshot.clone();
		match action {
			Action::Page(offset) => self.start(ctx, move || {
				Completion::Image(ImageCompletion::Runs(
					snapshot.run_page(offset).map(|page| (page, None)),
				))
			}),
			Action::LocateRun(offset) => self.start(ctx, move || {
				Completion::Image(ImageCompletion::Runs(
					snapshot
						.locate_run(offset)
						.map(|location| (location.page, Some(location.ordinal))),
				))
			}),
			Action::Verify => {
				let cancellation = Cancellation::default();
				self.start_worker(ctx, Some(cancellation.clone()), move || {
					Completion::Image(ImageCompletion::Checked(Box::new(
						snapshot.verify(&cancellation),
					)))
				});
			}
			Action::Export(kind) => self.start_image_export(ctx, kind),
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			Action::ShowContainer => {
				image.browser.showing = false;
				#[cfg(feature = "actual-preview")]
				{
					image.browser.preview = None;
				}
			}
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			Action::ShowFiles => image.browser.showing = true,
			#[cfg(feature = "actual-preview")]
			Action::ShowList => image.browser.preview = None,
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			Action::History(direction) => {
				if let Some(request) = image.browser.history.request(direction) {
					self.start_browse(ctx, snapshot, request, Some(direction));
				}
			}
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			Action::Browse(request) => {
				self.start_browse(ctx, snapshot, request, None);
			}
		}
	}

	pub(super) fn finish_image(&mut self, ctx: &Context, completion: ImageCompletion) {
		#[cfg(not(any(feature = "hfsplus-read", feature = "apfs-read")))]
		let _ = ctx;
		let Some(Opened {
			document: Document::Image(image),
			..
		}) = &mut self.opened
		else {
			return;
		};
		match completion {
			ImageCompletion::Runs(result) => match result {
				Ok((page, selected)) => {
					image.report.run_page = Some(page);
					image.run_navigation.select(selected);
				}
				Err(error) => {
					if matches!(error, Error::SourceChanged) {
						image.verification = None;
						image.run_navigation.select(None);
						#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
						image.browser.invalidate();
					}
					self.error = Some(format!("Could not load the run page. {error}"));
				}
			},
			ImageCompletion::Checked(report) => {
				if matches!(report.cause(), Some(Error::SourceChanged)) {
					image.run_navigation.select(None);
					#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
					image.browser.invalidate();
				}
				image.verification = Some(report.into());
			}
			ImageCompletion::Exported(result) => {
				Self::finish_image_export(image, &mut self.error, result);
			}
			#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
			ImageCompletion::Browsed { direction, result } => {
				self.finish_browse(ctx, direction, result);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use std::{fs, time::Duration};

	use super::*;
	use crate::app::open_outcome;

	const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
	const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

	fn apply_result(app: &mut App, ctx: &Context) {
		let result = app
			.worker
			.take()
			.expect("offset action starts a worker")
			.receiver
			.recv_timeout(WORKER_TIMEOUT)
			.expect("image worker delivers its completion");
		app.finish(ctx, result);
	}

	fn opened_image(app: &App) -> &crate::image::ImageTab {
		let Document::Image(image) = &app.opened.as_ref().unwrap().document else {
			panic!("expected image tab")
		};
		image
	}

	#[test]
	fn offset_lookup_loads_and_highlights_a_run_then_normal_paging_clears_it() {
		let ctx = Context::default();
		let scratch = tempfile::tempdir().unwrap();
		let path = scratch.path().join("retained.dmg");
		fs::write(&path, UDZO).unwrap();
		let mut app = App::new(&ctx, None);
		app.finish(&ctx, open_outcome(path.clone()));
		let container = &opened_image(&app).report.container;
		let last = container.runs.total - 1;
		let offset = container.logical_bytes - 1;
		app.image_action(&ctx, Action::LocateRun(offset));
		apply_result(&mut app, &ctx);
		assert_eq!(opened_image(&app).run_navigation.selected, Some(last));
		assert!(opened_image(&app).run_navigation.reveal_selected);

		app.image_action(&ctx, Action::LocateRun(offset + 1));
		apply_result(&mut app, &ctx);
		assert!(app.error.as_deref().unwrap().contains("exceeds disk size"));
		assert_eq!(opened_image(&app).run_navigation.selected, Some(last));

		app.image_action(&ctx, Action::Page(0));
		apply_result(&mut app, &ctx);
		assert_eq!(opened_image(&app).run_navigation.selected, None);
		assert!(!opened_image(&app).run_navigation.reveal_selected);
		assert!(app.error.is_none());

		app.image_action(&ctx, Action::LocateRun(offset));
		apply_result(&mut app, &ctx);
		fs::write(path, []).unwrap();
		app.image_action(&ctx, Action::Verify);
		apply_result(&mut app, &ctx);
		let image = opened_image(&app);
		let verification = image.verification.as_ref().unwrap();
		assert!(matches!(verification.cause(), Some(Error::SourceChanged)));
		assert_eq!(image.run_navigation.selected, None);
	}
}
