use std::sync::Arc;

use dmgez_udif::{Cancellation, Error};
use eframe::egui::Context;

use crate::image::{
	ImageSnapshot,
	browse::{self, Observation, Request, history::Direction},
};

use super::{App, Completion, Document, ImageCompletion, Opened};

impl App {
	pub(super) fn start_browse(
		&mut self,
		ctx: &Context,
		snapshot: Arc<ImageSnapshot>,
		request: Request,
		direction: Option<Direction>,
	) {
		let cancellation = Cancellation::default();
		self.start_worker(ctx, Some(cancellation.clone()), move || {
			Completion::Image(ImageCompletion::Browsed {
				direction,
				result: snapshot.browse(request, &cancellation),
			})
		});
	}

	pub(super) fn finish_browse(
		&mut self,
		ctx: &Context,
		direction: Option<Direction>,
		result: Result<Observation, browse::Error>,
	) {
		#[cfg(not(feature = "actual-preview"))]
		let _ = ctx;
		let Some(Opened {
			document: Document::Image(image),
			..
		}) = &mut self.opened
		else {
			return;
		};
		let browser = &mut image.browser;
		match result {
			#[cfg(feature = "apfs-read")]
			Ok(Observation::ApfsCatalog(catalog)) => browser.apfs = Some(catalog),
			Ok(Observation::Page(page)) => {
				#[cfg(feature = "actual-preview")]
				let next_preview = browser
					.preview
					.as_ref()
					.filter(|_| page.filesystem.supports_finder_preview())
					.map(|preview| Request::Preview {
						location: page.location.clone(),
						max_side: ctx.input(|input| input.max_texture_side),
						visibility: preview.visibility,
					});
				let arrived = browser.show_page(page, direction);
				#[cfg(feature = "actual-preview")]
				if arrived && let Some(request) = next_preview {
					let snapshot = Arc::clone(&image.snapshot);
					self.start_browse(ctx, snapshot, request, None);
				}
				#[cfg(not(feature = "actual-preview"))]
				let _ = arrived;
			}
			Ok(Observation::Details(details)) => browser.details = Some(details),
			Ok(Observation::Search(report)) => {
				if let Some(page) = &browser.page {
					browser.search.apply(&page.location, report);
				}
			}
			#[cfg(feature = "actual-preview")]
			Ok(Observation::Preview(preview)) => {
				if browser.showing
					&& browser
						.page
						.as_ref()
						.is_some_and(|page| page.location == preview.location)
				{
					match preview.present(ctx) {
						Ok(presented) => {
							browser.preview = Some(presented);
							browser.details = None;
						}
						Err(error) => self.error = Some(error.to_string()),
					}
				}
			}
			Err(error) => {
				if matches!(error, browse::Error::Image(Error::SourceChanged)) {
					image.verification = None;
					image.run_navigation.select(None);
					browser.invalidate();
				}
				self.error = Some(format!("Could not browse the volume. {error}"));
			}
		}
	}
}

#[cfg(all(test, feature = "actual-preview"))]
mod tests;
