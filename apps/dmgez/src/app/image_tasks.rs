use dmgez_inspector::{RunPage, VerificationReport};
use dmgez_udif::{Cancellation, Error};
use eframe::egui::Context;

#[cfg(feature = "hfsplus-read")]
use crate::image::browse::{self, Observation};
use crate::inspector::Action;

use super::{App, Completion, Document, Opened};

pub(super) enum ImageCompletion {
	Runs(Result<RunPage, Error>),
	Checked(Box<VerificationReport>),
	#[cfg(feature = "hfsplus-read")]
	Browsed(Result<Observation, browse::Error>),
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
				Completion::Image(ImageCompletion::Runs(snapshot.run_page(offset)))
			}),
			Action::Verify => {
				let cancellation = Cancellation::default();
				self.start_worker(ctx, Some(cancellation.clone()), move || {
					Completion::Image(ImageCompletion::Checked(Box::new(
						snapshot.verify(&cancellation),
					)))
				});
			}
			#[cfg(feature = "hfsplus-read")]
			Action::ShowContainer => {
				image.browser.showing = false;
				#[cfg(feature = "actual-preview")]
				{
					image.browser.preview = None;
				}
			}
			#[cfg(feature = "hfsplus-read")]
			Action::ShowFiles => image.browser.showing = true,
			#[cfg(feature = "actual-preview")]
			Action::ShowList => image.browser.preview = None,
			#[cfg(feature = "hfsplus-read")]
			Action::Browse(request) => {
				let cancellation = Cancellation::default();
				self.start_worker(ctx, Some(cancellation.clone()), move || {
					Completion::Image(ImageCompletion::Browsed(
						snapshot.browse(request, &cancellation),
					))
				});
			}
		}
	}

	pub(super) fn finish_image(&mut self, ctx: &Context, completion: ImageCompletion) {
		#[cfg(not(feature = "actual-preview"))]
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
				Ok(page) => image.report.run_page = Some(page),
				Err(error) => {
					if matches!(error, Error::SourceChanged) {
						image.verification = None;
						#[cfg(feature = "hfsplus-read")]
						image.browser.invalidate();
					}
					self.error = Some(format!("Could not load the run page. {error}"));
				}
			},
			ImageCompletion::Checked(report) => {
				#[cfg(feature = "hfsplus-read")]
				if matches!(report.cause(), Some(Error::SourceChanged)) {
					image.browser.invalidate();
				}
				image.verification = Some(report);
			}
			#[cfg(feature = "hfsplus-read")]
			ImageCompletion::Browsed(result) => match result {
				Ok(Observation::Page(page)) => image.browser.show_page(page),
				Ok(Observation::Details(details)) => image.browser.details = Some(details),
				#[cfg(feature = "actual-preview")]
				Ok(Observation::Preview(preview)) => {
					if image.browser.showing
						&& image
							.browser
							.page
							.as_ref()
							.is_some_and(|page| page.location == preview.location)
					{
						match preview.present(ctx) {
							Ok(presented) => {
								image.browser.preview = Some(presented);
								image.browser.details = None;
							}
							Err(error) => self.error = Some(error.to_string()),
						}
					}
				}
				Err(error) => {
					if matches!(error, browse::Error::Image(Error::SourceChanged)) {
						image.verification = None;
						image.browser.invalidate();
					}
					self.error = Some(format!("Could not browse the volume. {error}"));
				}
			},
		}
	}
}
