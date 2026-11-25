use std::{fs, sync::mpsc, time::Duration};

use dmgez_diff::Outcome;
use dmgez_runtime::compare::ByteSpace;
use dmgez_udif::writer::{Profile, WriteOptions, write_image};
use eframe::egui::{self, accesskit};

use super::*;
use crate::app::open_outcome;

const UDRO: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../../tests/fixtures/images/tiny-UDZO.dmg");
const RAW: &[u8] = include_bytes!("../../../../../tests/fixtures/expected/tiny.raw");
const TIMEOUT: Duration = Duration::from_secs(5);

fn finish_worker(app: &mut App, ctx: &Context) {
	let worker = app
		.worker
		.take()
		.expect("comparison action started a worker");
	let completed = worker
		.receiver
		.recv_timeout(TIMEOUT)
		.expect("worker completed");
	app.finish(ctx, completed);
}

fn start(app: &mut App, ctx: &Context, path: &Path, request: Request) {
	let other = path.to_path_buf();
	app.start_comparison(ctx, request, move |_| Some(other));
	finish_worker(app, ctx);
}

fn outcome(app: &App) -> Outcome {
	let completed = app.comparison.result.as_ref().unwrap().as_ref().unwrap();
	match completed.report {
		Report::Bytes { report, .. } => report.outcome,
		#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
		Report::Filesystem(_) => panic!("expected a byte comparison"),
	}
}

fn open_image(ctx: &Context, scratch: &Path, bytes: &[u8]) -> (App, PathBuf) {
	let path = scratch.join("opened.dmg");
	fs::write(&path, bytes).unwrap();
	let mut app = App::new(ctx, None);
	app.finish(ctx, open_outcome(path.clone()));
	(app, path)
}

#[test]
fn comparison_uses_the_open_image_and_the_selected_address_space() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, source) = open_image(&ctx, scratch.path(), UDZO);
	let other = scratch.path().join("uncompressed.dmg");
	fs::write(&other, UDRO).unwrap();
	start(&mut app, &ctx, &other, Request::logical());
	assert_eq!(outcome(&app), Outcome::Equal);
	start(&mut app, &ctx, &other, Request::Bytes(ByteSpace::Physical));
	assert_eq!(outcome(&app), Outcome::ByteMismatch);
	assert_eq!(fs::read(&source).unwrap(), UDZO);
	assert_eq!(fs::read(&other).unwrap(), UDRO);

	// A later open at the same path could compare successfully. The retained
	// source must reject this mutation instead.
	fs::write(&source, UDRO).unwrap();
	start(&mut app, &ctx, &other, Request::logical());
	assert!(app.comparison.result.as_ref().unwrap().is_err());
	app.finish(&ctx, open_outcome(other));
	assert!(app.comparison.result.is_none());
}

#[test]
fn comparison_can_be_cancelled_while_the_picker_is_open() {
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, path) = open_image(&ctx, scratch.path(), UDZO);
	let (send, receive) = mpsc::sync_channel(1);
	app.start_comparison(&ctx, Request::logical(), move |_| {
		receive.recv_timeout(TIMEOUT).ok()
	});
	let worker = app.worker.as_ref().unwrap();
	worker.cancellation.as_ref().unwrap().cancel();
	send.send(path).unwrap();
	finish_worker(&mut app, &ctx);
	assert!(app.comparison.result.is_none());
}

#[test]
fn accessible_comparison_result_can_locate_the_first_differing_run() {
	let ctx = Context::default();
	ctx.enable_accesskit();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, source) = open_image(&ctx, scratch.path(), UDZO);
	let other = scratch.path().join("changed.dmg");
	let mut different = RAW.to_vec();
	let first_difference = (2 * dmgez_udif::SECTOR_BYTES + 3) as usize;
	different[first_difference] ^= 1;
	write_image(
		different.as_slice(),
		different.len() as u64,
		fs::File::create(&other).unwrap(),
		&WriteOptions {
			profile: Profile::Udro,
			..WriteOptions::default()
		},
	)
	.unwrap();
	start(&mut app, &ctx, &other, Request::logical());
	assert_eq!(outcome(&app), Outcome::ByteMismatch);
	app.comparison.open = true;
	let input = egui::RawInput {
		screen_rect: Some(egui::Rect::from_min_size(
			egui::Pos2::ZERO,
			crate::MINIMUM_SIZE.into(),
		)),
		..egui::RawInput::default()
	};
	let render = |input, app: &mut App| {
		let mut output = ctx.run_ui(input, |_| app.show_comparison(&ctx));
		let accessibility = output.platform_output.accesskit_update.take();
		output.drop_without_applying_deltas();
		accessibility
	};
	let _ = render(input.clone(), &mut app);
	let tree = render(input, &mut app).unwrap();
	let (target, _) = tree
		.nodes
		.iter()
		.find(|(_, node)| node.label() == Some("Go to first differing run"))
		.expect("compare result has an accessible run action");
	let action = egui::RawInput {
		events: vec![egui::Event::AccessKitActionRequest(
			accesskit::ActionRequest {
				action: accesskit::Action::Click,
				target_tree: accesskit::TreeId::ROOT,
				target_node: *target,
				data: None,
			},
		)],
		..egui::RawInput::default()
	};
	let _ = render(action, &mut app);
	finish_worker(&mut app, &ctx);
	let Document::Image(image) = &app.opened.as_ref().unwrap().document else {
		panic!("expected the open image after navigation");
	};
	let expected = image.snapshot.locate_run(first_difference as u64).unwrap();
	assert_eq!(image.run_navigation.selected, Some(expected.ordinal));
	assert!(image.run_navigation.reveal_selected);
	assert_eq!(fs::read(source).unwrap(), UDZO);
}

#[cfg(feature = "hfsplus-read")]
#[test]
fn filesystem_comparison_reports_selected_hfs_windows_and_changed_paths() {
	use dmgez_diff::semantic::{self, Options};

	use crate::image::compare::Selection;

	const APM: &[u8] =
		include_bytes!("../../../../../crates/foundation/dmgez-vfs/tests/fixtures/native-apm.dmg");
	const FINDER: &[u8] =
		include_bytes!("../../../../../tests/fixtures/images/actual-preview-udzo.dmg");
	let ctx = Context::default();
	let scratch = tempfile::tempdir().unwrap();
	let (mut app, source) = open_image(&ctx, scratch.path(), APM);
	let other = scratch.path().join("other.dmg");
	fs::write(&other, FINDER).unwrap();
	start(
		&mut app,
		&ctx,
		&other,
		Request::HfsPlus {
			left: Selection {
				partition: std::num::NonZeroU32::new(2),
				#[cfg(feature = "apfs-read")]
				volume: None,
				#[cfg(feature = "apfs-read")]
				checkpoint: None,
			},
			right: Selection::default(),
			options: Options::default(),
		},
	);
	let completed = app.comparison.result.as_ref().unwrap().as_ref().unwrap();
	let Report::Filesystem(report) = &completed.report else {
		panic!("expected a filesystem comparison");
	};
	assert!(report.left.contains("Partition 2"));
	assert!(report.right.contains("Whole disk"));
	assert_eq!(report.comparison.outcome, semantic::Outcome::Different);
	assert_eq!(report.comparison.left_entries, 1);
	assert!(report.comparison.right_entries > 1);
	assert!(
		report.comparison.changes.iter().any(|change| {
			change.path == ["Guide"] && change.kind == semantic::ChangeKind::Added
		})
	);
	assert_eq!(fs::read(source).unwrap(), APM);
	assert_eq!(fs::read(other).unwrap(), FINDER);
}
