use std::io::Write as _;

use super::*;
use crate::image::browse::content::png::Issue;

const IMAGE: &[u8] =
	include_bytes!("../../../../../../tests/fixtures/images/actual-preview-udzo.dmg");
const ARTWORK_DIRECTORY: &str = ".background";
const EXPECTED_SIZE: [usize; 2] = [8, 8];
const EXPECTED_COLOR: egui::Color32 = egui::Color32::from_rgb(23, 77, 129);

#[test]
fn accessible_png_preview_shows_the_selected_embedded_file() {
	let mut image = tempfile::tempfile().unwrap();
	image.write_all(IMAGE).unwrap();
	let mut tab = ImageTab::open(image).unwrap();
	let root = Location::root(VolumeId::WholeDisk);
	let folder = root.child(&utf16(ARTWORK_DIRECTORY)).unwrap();
	let selected = page(&tab, folder.clone(), 0);
	assert_eq!(selected.entries.len(), 1);
	let name = selected.entries[0].name.clone();
	tab.browser.showing = true;
	assert!(tab.browser.show_page(selected, None));
	let Observation::Details(details) = tab
		.snapshot
		.browse(
			Request::Details {
				folder: folder.clone(),
				name: name.clone(),
			},
			&Cancellation::default(),
		)
		.unwrap()
	else {
		panic!("expected file details");
	};
	assert!(details.png.is_none());
	let native_id = details.native_id;
	tab.browser.details = Some(details);
	let Action::Browse(request @ Request::Png { .. }) = activate(&mut tab, "Preview PNG") else {
		panic!("expected an explicit PNG preview request");
	};
	let Observation::Png(prepared) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected decoded PNG pixels");
	};
	assert_eq!(prepared.details.folder, folder);
	assert_eq!(prepared.details.name, name);
	assert_eq!(prepared.details.native_id, native_id);
	let pixels = prepared.image.as_ref().unwrap();
	assert_eq!(pixels.size, EXPECTED_SIZE);
	assert!(pixels.pixels.iter().all(|pixel| *pixel == EXPECTED_COLOR));

	let context = egui::Context::default();
	context.enable_accesskit();
	let displayed = prepared.present(&context);
	let texture = &displayed.png.as_deref().unwrap().as_ref().unwrap().texture;
	assert_eq!(texture.size(), EXPECTED_SIZE);
	tab.browser.details = Some(displayed);
	let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::INITIAL_SIZE.into());
	let mut output = context.run_ui(
		egui::RawInput {
			screen_rect: Some(viewport),
			..Default::default()
		},
		|ui| assert!(inspector::show(ui, &mut tab, true).is_none()),
	);
	let update = output.platform_output.accesskit_update.take().unwrap();
	output.drop_without_applying_deltas();
	assert!(
		update
			.nodes
			.iter()
			.any(|(_, node)| { node.label() == Some("Decoded PNG file preview") })
	);
	let root_page = page(&tab, root, 0);
	assert!(tab.browser.show_page(root_page, None));
	assert!(tab.browser.details.is_none());
}

#[test]
fn png_preview_distinguishes_non_png_files_and_honors_cancellation() {
	let tab = wrap(&fixture(CATALOG));
	let folder = Location::root(VolumeId::WholeDisk)
		.child(&utf16("Nested"))
		.unwrap();
	let selected = page(&tab, folder.clone(), 0);
	let request = || Request::Png {
		folder: folder.clone(),
		name: selected.entries[0].name.clone(),
	};
	let Observation::Png(prepared) = tab
		.snapshot
		.browse(request(), &Cancellation::default())
		.unwrap()
	else {
		panic!("expected PNG preview result");
	};
	assert!(matches!(prepared.image, Err(Issue::UnsupportedFormat)));
	let cancellation = Cancellation::default();
	cancellation.cancel();
	assert!(matches!(
		tab.snapshot.browse(request(), &cancellation),
		Err(Error::Image(ImageError::Cancelled))
	));
}

fn utf16(name: &str) -> RawName {
	RawName::Utf16(name.encode_utf16().collect())
}
