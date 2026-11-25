use std::{fs, path::PathBuf, time::Duration};

use dmgez_preview::ItemVisibility;
use dmgez_udif::writer::{Profile, WriteOptions, write_image};
use dmgez_vfs::{VolumeId, volume::RawName};

use super::*;
use crate::{image::browse::Location, inspector::Action};

const FIXTURES: &str = "DMGEZ_HFSPLUS_FIXTURES";
const CATALOG: &str = "native-catalog-volume.bin";
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);

fn finish_one(app: &mut App, ctx: &Context) {
	let worker = app.worker.take().expect("a browse worker is running");
	let result = worker.receiver.recv_timeout(WORKER_TIMEOUT).unwrap();
	drop(worker);
	app.finish(ctx, result);
}

fn browser(app: &App) -> &browse::Browser {
	let Document::Image(image) = &app.opened.as_ref().unwrap().document else {
		panic!("expected image tab");
	};
	&image.browser
}

#[test]
fn navigation_reopens_the_actual_preview_with_the_same_visibility() {
	let directory = PathBuf::from(std::env::var_os(FIXTURES).unwrap());
	let raw = fs::read(directory.join(CATALOG)).unwrap();
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().join("catalog.dmg");
	write_image(
		raw.as_slice(),
		raw.len() as u64,
		fs::File::create(&path).unwrap(),
		&WriteOptions {
			profile: Profile::Udro,
			..WriteOptions::default()
		},
	)
	.unwrap();
	let ctx = Context::default();
	let mut app = App::new(&ctx, None);
	app.finish(&ctx, crate::app::open_outcome(path));
	app.image_action(&ctx, Action::ShowFiles);

	let root = Location::root(VolumeId::WholeDisk);
	app.image_action(&ctx, Action::Browse(Request::first(root.clone())));
	finish_one(&mut app, &ctx);
	let visibility = ItemVisibility::IncludeHidden;
	app.image_action(
		&ctx,
		Action::Browse(Request::Preview {
			location: root,
			max_side: ctx.input(|input| input.max_texture_side),
			visibility,
		}),
	);
	finish_one(&mut app, &ctx);
	assert!(browser(&app).preview.is_some());

	let name = RawName::Utf16("Nested".encode_utf16().collect());
	let child = browser(&app)
		.page
		.as_ref()
		.unwrap()
		.location
		.child(&name)
		.unwrap();
	app.image_action(&ctx, Action::Browse(Request::first(child.clone())));
	finish_one(&mut app, &ctx);
	assert_eq!(browser(&app).page.as_ref().unwrap().location, child);
	assert!(browser(&app).preview.is_none());
	finish_one(&mut app, &ctx);
	assert_eq!(
		browser(&app).preview.as_ref().unwrap().visibility,
		visibility
	);
	assert!(browser(&app).history.available(Direction::Back));
}
