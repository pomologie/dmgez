use std::{fs, io::Write as _, path::PathBuf};

use dmgez_preview::{
	actual::{BackgroundAsset, Entry, EntryKind, Store},
	raster::ActualPreview,
};
use dmgez_udif::{
	Cancellation,
	writer::{Profile, WriteOptions, write_image},
};
use dmgez_vfs::{VolumeId, volume::RawName};
use eframe::egui::{
	Context, Event, Key, Modifiers, PointerButton, RawInput, accesskit, pos2, vec2,
};

use super::*;
use crate::image::{
	ImageTab,
	browse::{Observation, preview::Icon},
};

const ACTUAL_DMG: &[u8] =
	include_bytes!("../../../../../../tests/fixtures/images/actual-preview-udzo.dmg");

#[test]
fn actual_preview_icons_follow_recorded_geometry_and_hidden_visibility() {
	let mut image = tempfile::tempfile().unwrap();
	image.write_all(ACTUAL_DMG).unwrap();
	let tab = ImageTab::open(image).unwrap();
	let context = Context::default();
	context.enable_accesskit();
	let location = Location::root(VolumeId::WholeDisk);
	let presented = prepare(&tab, &context, &location, ItemVisibility::Visible);
	let (_, nodes) = frame(&context, &location, &presented, vec![]);
	assert!(
		nodes
			.iter()
			.all(|(_, node)| node.label() != Some("Open folder Private"))
	);
	let (_, texture) = find(&nodes, PREVIEW_ALT_TEXT);
	let (_, guide) = find(&nodes, "Open folder Guide");
	assert!(guide.supports_action(accesskit::Action::Click));
	let pixels = texture.bounds().unwrap();
	let icon = guide.bounds().unwrap();
	// The independent fixture oracle records a 20-point icon centered at (40, 25).
	assert_eq!(icon.x0 - pixels.x0, 30.0);
	assert_eq!(icon.y0 - pixels.y0, 15.0);
	assert_eq!(icon.x1 - icon.x0, 20.0);
	assert_eq!(icon.y1 - icon.y0, 20.0);
	let pointer = pos2(
		icon.x0.midpoint(icon.x1) as f32,
		icon.y0.midpoint(icon.y1) as f32,
	);
	let press = |pressed| Event::PointerButton {
		pos: pointer,
		button: PointerButton::Primary,
		pressed,
		modifiers: Modifiers::NONE,
	};
	frame(
		&context,
		&location,
		&presented,
		vec![Event::PointerMoved(pointer), press(true)],
	);
	let (action, _) = frame(&context, &location, &presented, vec![press(false)]);
	let Some(Action::Browse(request @ Request::Page { .. })) = action else {
		panic!("clicking Guide should open the folder");
	};
	let Observation::Page(page) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("Guide should produce its exact catalog page");
	};
	assert_eq!(page.location.path, [name("Guide")]);
	assert!(
		page.entries
			.iter()
			.any(|entry| entry.name == name("Introduction"))
	);

	let (hidden, _) = find(&nodes, "Show hidden items");
	let (action, _) = frame(
		&context,
		&location,
		&presented,
		vec![access_request(hidden, accesskit::Action::Click)],
	);
	let Some(Action::Browse(
		request @ Request::Preview {
			visibility: ItemVisibility::IncludeHidden,
			..
		},
	)) = action
	else {
		panic!("the accessible hidden control should request an updated preview");
	};
	let Observation::Preview(prepared) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("the visibility request should return a rendered preview");
	};
	let with_hidden = prepared.present(&context).unwrap();
	let (_, nodes) = frame(&context, &location, &with_hidden, vec![]);
	let (private, _) = find(&nodes, "Open folder Private");
	let (action, _) = frame(
		&context,
		&location,
		&with_hidden,
		vec![access_request(private, accesskit::Action::Click)],
	);
	assert!(
		matches!(action, Some(Action::Browse(Request::Page { location, offset: 0 })) if location.path == [name("Private")])
	);
}

#[test]
fn keyboard_activation_inspects_the_exact_stored_utf16_name() {
	let fixtures = std::env::var_os("DMGEZ_HFSPLUS_FIXTURES")
		.expect("desktop composition supplies HFS+ fixtures");
	let raw = fs::read(PathBuf::from(fixtures).join("native-catalog-volume.bin")).unwrap();
	let mut image = tempfile::tempfile().unwrap();
	write_image(
		raw.as_slice(),
		raw.len() as u64,
		&mut image,
		&WriteOptions {
			profile: Profile::Udro,
			..WriteOptions::default()
		},
	)
	.unwrap();
	let tab = ImageTab::open(image).unwrap();
	let context = Context::default();
	context.enable_accesskit();
	let location = Location::root(VolumeId::WholeDisk)
		.child(&name("Nested"))
		.unwrap();
	let presented = prepare(&tab, &context, &location, ItemVisibility::Visible);
	let (_, nodes) = frame(&context, &location, &presented, vec![]);
	let stored = "Re\u{301}sume\u{301}.txt";
	let caption = format!("Inspect {}", label(&name(stored)));
	let (file, _) = find(&nodes, &caption);
	frame(
		&context,
		&location,
		&presented,
		vec![access_request(file, accesskit::Action::Focus)],
	);
	let (action, _) = frame(
		&context,
		&location,
		&presented,
		vec![Event::Key {
			key: Key::Enter,
			physical_key: None,
			pressed: true,
			repeat: false,
			modifiers: Modifiers::NONE,
		}],
	);
	let Some(Action::Browse(request @ Request::Details { .. })) = action else {
		panic!("Enter on a preview file should inspect it");
	};
	let Observation::Details(details) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("the exact stored name should resolve from the preview");
	};
	assert_eq!(details.name, name(stored));
}

#[test]
fn application_directories_and_symbolic_links_only_open_catalog_details() {
	let location = Location::root(VolumeId::WholeDisk);
	for (stored, kind) in [
		("Re\u{301}sume\u{301}.APP", EntryKind::Directory),
		("Alias", EntryKind::Symlink),
	] {
		let items = ActualPreview::from_image(
			&[Entry {
				name: stored.encode_utf16().collect(),
				kind,
				hidden: false,
				hide_extension: false,
			}],
			Store::Absent,
			BackgroundAsset::Unavailable,
			ItemVisibility::Visible,
		)
		.unwrap()
		.into_items();
		let icon = Icon::from(items.into_iter().next().unwrap());
		assert!(!icon.opens_folder());
		let Some(Request::Details {
			folder,
			name: observed,
		}) = icon.request(&location)
		else {
			panic!("requested details for {stored}");
		};
		assert_eq!(folder, location);
		assert_eq!(observed, name(stored));
	}
}

fn name(text: &str) -> RawName {
	RawName::Utf16(text.encode_utf16().collect())
}

fn prepare(
	tab: &ImageTab,
	context: &Context,
	location: &Location,
	visibility: ItemVisibility,
) -> Presented {
	let request = Request::Preview {
		location: location.clone(),
		max_side: context.input(|input| input.max_texture_side),
		visibility,
	};
	let Observation::Preview(prepared) = tab
		.snapshot
		.browse(request, &Cancellation::default())
		.unwrap()
	else {
		panic!("expected the folder preview");
	};
	prepared.present(context).unwrap()
}

fn frame(
	context: &Context,
	location: &Location,
	presented: &Presented,
	events: Vec<Event>,
) -> (Option<Action>, Vec<(accesskit::NodeId, accesskit::Node)>) {
	let mut action = None;
	let mut output = context.run_ui(
		RawInput {
			screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(960.0, 640.0))),
			events,
			..RawInput::default()
		},
		|ui| {
			let emitted = show(ui, location, Some(presented));
			action = action.take().or(emitted);
		},
	);
	let nodes = output
		.platform_output
		.accesskit_update
		.take()
		.map(|update| update.nodes)
		.unwrap_or_default();
	output.drop_without_applying_deltas();
	(action, nodes)
}

fn find<'a>(
	nodes: &'a [(accesskit::NodeId, accesskit::Node)],
	label: &str,
) -> (accesskit::NodeId, &'a accesskit::Node) {
	nodes
		.iter()
		.find_map(|(id, node)| (node.label() == Some(label)).then_some((*id, node)))
		.unwrap_or_else(|| panic!("missing accessible node {label}"))
}

fn access_request(target_node: accesskit::NodeId, action: accesskit::Action) -> Event {
	Event::AccessKitActionRequest(accesskit::ActionRequest {
		action,
		target_tree: accesskit::TreeId::ROOT,
		target_node,
		data: None,
	})
}
