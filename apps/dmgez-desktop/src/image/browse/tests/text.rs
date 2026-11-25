use super::*;
use crate::image::browse::content::{Extent, Preview};

#[test]
fn accessible_text_preview_reads_exact_native_file_and_preserves_details() {
	let mut tab = wrap(&fixture(CATALOG));
	let root = Location::root(VolumeId::WholeDisk);
	let folder = root.child(&utf16("Nested")).unwrap();
	let selected = page(&tab, folder.clone(), 0);
	let name = selected.entries[0].name.clone();
	tab.browser.showing = true;
	assert!(tab.browser.show_page(selected, None));
	let details = inspect(
		&tab,
		Request::Details {
			folder: folder.clone(),
			name: name.clone(),
		},
	);
	assert!(details.text.is_none());
	let catalog_id = details.native_id;
	let file_mode = details.metadata.mode;
	tab.browser.details = Some(details);

	let Action::Browse(request @ Request::Text { .. }) = activate(&mut tab, "Preview text") else {
		panic!("expected an explicit text preview request");
	};
	let details = inspect(&tab, request);
	assert_eq!(details.folder, folder);
	assert_eq!(details.name, name);
	assert_eq!(details.native_id, catalog_id);
	assert_eq!(details.metadata.mode, file_mode);
	let expected = std::str::from_utf8(NATIVE_NESTED_PAYLOAD).unwrap();
	assert_eq!(
		details.text,
		Some(Preview::Text {
			value: expected.into(),
			extent: Extent::Complete,
		})
	);
	tab.browser.details = Some(details);
	let context = egui::Context::default();
	context.enable_accesskit();
	let viewport = egui::Rect::from_min_size(
		egui::Pos2::ZERO,
		egui::vec2(crate::INITIAL_SIZE[0], 1_600.0),
	);
	let mut output = context.run_ui(
		egui::RawInput {
			screen_rect: Some(viewport),
			..Default::default()
		},
		|ui| assert!(inspector::show(ui, &mut tab, true).is_none()),
	);
	let update = output.platform_output.accesskit_update.take().unwrap();
	output.drop_without_applying_deltas();
	for label in ["Complete UTF-8 text", expected.trim_end()] {
		assert!(
			update.nodes.iter().any(|(_, node)| {
				[node.label(), node.value()]
					.into_iter()
					.flatten()
					.any(|text| text.trim_end() == label)
			}),
			"text preview should expose {label}"
		);
	}
}

#[test]
fn hfsx_text_requests_distinguish_binary_content_and_refuse_case_aliases() {
	let tab = wrap(&materialize_sparse(&fixture(HFSX)));
	let root = Location::root(VolumeId::WholeDisk);
	let folder = root.child(&utf16("Cases")).unwrap();
	let details = inspect(
		&tab,
		Request::Text {
			folder: folder.clone(),
			name: utf16("Case.txt"),
		},
	);
	assert_eq!(details.name, utf16("Case.txt"));
	assert_eq!(details.text, Some(Preview::Binary));
	assert!(matches!(
		tab.snapshot.browse(
			Request::Text {
				folder,
				name: utf16("CASE.txt"),
			},
			&Cancellation::default()
		),
		Err(Error::EntryChanged)
	));

	let directory = inspect(
		&tab,
		Request::Text {
			folder: root,
			name: utf16("Cases"),
		},
	);
	assert_eq!(directory.text, Some(Preview::UnsupportedKind));
}

fn inspect(tab: &ImageTab, request: Request) -> Details {
	let observation = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap();
	let Observation::Details(details) = observation else {
		panic!("expected item details");
	};
	details
}

fn utf16(name: &str) -> RawName {
	RawName::Utf16(name.encode_utf16().collect())
}
