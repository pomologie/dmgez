use std::{
	fs,
	io::{self, Read},
};

use dmgez_inspector::{CheckStatus, ErrorCategory, VerificationOutcome};
use dmgez_udif::{
	SECTOR_BYTES,
	writer::{Profile, WriteOptions, write_image},
};
use eframe::egui::{self, accesskit};

use super::*;
use crate::inspector::{self, Action};

const NATIVE_UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const NATIVE_SIGNED: &[u8] =
	include_bytes!("../../../../tests/fixtures/images/tiny-UDZO-adhoc-sha256.dmg");
const NATIVE_ULFO: &[u8] =
	include_bytes!("../../../../crates/formats/dmgez-udif/tests/fixtures/apple-ulfo.dmg");
const NATIVE_ULMO: &[u8] =
	include_bytes!("../../../../crates/formats/dmgez-udif/tests/fixtures/apple-ulmo.dmg");
const IMAGE_NAME: &str = "snapshot.dmg";

const LAST_PAGE_ROWS: usize = 3;

fn multi_page_image() -> ImageTab {
	let mut stored = tempfile::tempfile().unwrap();
	let logical_bytes = (2 * RUN_PAGE_SIZE + LAST_PAGE_ROWS) as u64 * SECTOR_BYTES;
	write_image(
		io::repeat(b'r').take(logical_bytes),
		logical_bytes,
		&mut stored,
		&WriteOptions {
			profile: Profile::Udro,
			chunk_bytes: SECTOR_BYTES as usize,
			..WriteOptions::default()
		},
	)
	.unwrap();
	ImageTab::open(stored).unwrap()
}

#[test]
fn image_run_pages_use_the_retained_index() {
	let tab = multi_page_image();
	let first = tab.report.run_page.unwrap();
	assert_eq!(first.runs.len(), RUN_PAGE_SIZE);
	let second = tab.snapshot.run_page(first.next_offset.unwrap()).unwrap();
	assert_eq!(second.offset, RUN_PAGE_SIZE as u64);
	assert_eq!(second.runs.len(), RUN_PAGE_SIZE);
	let last = tab.snapshot.run_page(second.next_offset.unwrap()).unwrap();
	assert_eq!(last.runs.len(), LAST_PAGE_ROWS);
	assert_eq!(
		last.runs[0].logical.offset_bytes,
		last.offset * SECTOR_BYTES
	);
	assert_eq!(last.next_offset, None);
	assert_eq!(tab.snapshot.run_page(0).unwrap(), first);
}

#[test]
fn logical_offsets_select_the_run_that_contains_the_requested_byte() {
	let tab = multi_page_image();
	let disk_len = tab.report.container.logical_bytes;
	let last_ordinal = 2 * RUN_PAGE_SIZE + LAST_PAGE_ROWS - 1;
	let first_page_end = RUN_PAGE_SIZE as u64 * SECTOR_BYTES;
	for (offset, ordinal, page_offset) in [
		(0, 0, 0),
		(first_page_end - 1, RUN_PAGE_SIZE - 1, 0),
		(first_page_end, RUN_PAGE_SIZE, RUN_PAGE_SIZE),
		(disk_len - 1, last_ordinal, 2 * RUN_PAGE_SIZE),
	] {
		let location = tab.snapshot.locate_run(offset).unwrap();
		assert_eq!(location.ordinal, ordinal);
		assert_eq!(location.page.offset, page_offset as u64);
		let run = &location.page.runs[ordinal - page_offset];
		assert!(
			(run.logical.offset_bytes..run.logical.offset_bytes + run.logical.length_bytes)
				.contains(&offset)
		);
	}
	for offset in [disk_len, u64::MAX] {
		assert!(matches!(
			tab.snapshot.locate_run(offset),
			Err(Error::OutOfRange { offset: rejected, len: 1, .. }) if rejected == offset
		));
	}
}

#[test]
fn assistive_actions_reach_the_run_pager_and_container_check() {
	let mut tab = multi_page_image();
	for label in ["Next page", "Verify container"] {
		let ctx = egui::Context::default();
		ctx.enable_accesskit();
		let initial = egui::RawInput {
			screen_rect: Some(egui::Rect::from_min_size(
				egui::Pos2::ZERO,
				crate::MINIMUM_SIZE.into(),
			)),
			..egui::RawInput::default()
		};
		let mut output = ctx.run_ui(initial, |ui| {
			assert!(inspector::show(ui, &mut tab, true).is_none())
		});
		let accessibility = output.platform_output.accesskit_update.take();
		output.drop_without_applying_deltas();
		let target_node = accessibility
			.unwrap()
			.nodes
			.into_iter()
			.find(|(_, node)| {
				node.label() == Some(label) && node.supports_action(accesskit::Action::Click)
			})
			.unwrap_or_else(|| panic!("missing accessible {label} action"))
			.0;
		let input = egui::RawInput {
			events: vec![egui::Event::AccessKitActionRequest(
				accesskit::ActionRequest {
					action: accesskit::Action::Click,
					target_tree: accesskit::TreeId::ROOT,
					target_node,
					data: None,
				},
			)],
			..egui::RawInput::default()
		};
		let mut action = None;
		ctx.run_ui(input, |ui| {
			if let Some(emitted) = inspector::show(ui, &mut tab, true) {
				action = Some(emitted);
			}
		})
		.drop_without_applying_deltas();
		assert!(matches!(
			(label, action),
			("Next page", Some(Action::Page(RUN_PAGE_SIZE)))
				| ("Verify container", Some(Action::Verify))
		));
	}
}

#[test]
fn logical_offset_field_is_labelled_and_submits_with_enter() {
	let mut tab = multi_page_image();
	let ctx = egui::Context::default();
	ctx.enable_accesskit();
	let initial = egui::RawInput {
		screen_rect: Some(egui::Rect::from_min_size(
			egui::Pos2::ZERO,
			crate::MINIMUM_SIZE.into(),
		)),
		..egui::RawInput::default()
	};
	let mut output = ctx.run_ui(initial, |ui| {
		assert!(inspector::show(ui, &mut tab, true).is_none());
	});
	let accesskit = output.platform_output.accesskit_update.take().unwrap();
	output.drop_without_applying_deltas();
	let (text_field, node) = accesskit
		.nodes
		.iter()
		.find(|(_, node)| node.supports_action(accesskit::Action::SetValue))
		.expect("accessible logical offset field");
	assert!(accesskit.nodes.iter().any(|(id, label)| {
		node.labelled_by().contains(id) && label.value() == Some("Logical byte offset")
	}));
	let request = |action, data| {
		egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
			action,
			target_tree: accesskit::TreeId::ROOT,
			target_node: *text_field,
			data,
		})
	};
	for (value, expected) in [
		("0x200", Some(SECTOR_BYTES)),
		("18446744073709551615000", None),
		("1024", Some(SECTOR_BYTES * 2)),
	] {
		let input = egui::RawInput {
			events: vec![
				request(accesskit::Action::Focus, None),
				request(
					accesskit::Action::SetValue,
					Some(accesskit::ActionData::Value(value.into())),
				),
			],
			..egui::RawInput::default()
		};
		ctx.run_ui(input, |ui| {
			assert!(inspector::show(ui, &mut tab, true).is_none());
		})
		.drop_without_applying_deltas();
		let enter = egui::RawInput {
			events: vec![egui::Event::Key {
				key: egui::Key::Enter,
				physical_key: None,
				pressed: true,
				repeat: false,
				modifiers: egui::Modifiers::NONE,
			}],
			..egui::RawInput::default()
		};
		let mut action = None;
		ctx.run_ui(enter, |ui| {
			if let Some(emitted) = inspector::show(ui, &mut tab, true) {
				action = Some(emitted);
			}
		})
		.drop_without_applying_deltas();
		assert_eq!(
			tab.run_navigation.invalid_offset,
			expected.is_none(),
			"{value}"
		);
		assert_eq!(
			action.map(|action| match action {
				Action::LocateRun(offset) => offset,
				_ => panic!("unexpected action"),
			}),
			expected,
			"{value}"
		);
	}
}

#[test]
fn verification_uses_container_scope_and_reports_a_corrupt_payload() {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().join(IMAGE_NAME);
	let mut data = NATIVE_UDZO.to_vec();
	for expected in [VerificationOutcome::Passed, VerificationOutcome::Failed] {
		fs::write(&path, &data).unwrap();
		let tab = ImageTab::open(File::open(&path).unwrap()).unwrap();
		assert_eq!(tab.report.coverage.payload, CheckStatus::NotChecked);
		let result = tab.snapshot.verify(&Cancellation::default());
		assert_eq!(result.outcome, expected);
		assert_eq!(result.coverage.structure, CheckStatus::Passed);
		assert_eq!(result.coverage.filesystem, CheckStatus::NotChecked);
		assert_eq!(result.coverage.security, CheckStatus::NotChecked);
		data[0] ^= 1;
	}
}

#[test]
fn opens_a_signed_snapshot_and_keeps_security_coverage_independent() {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().join(IMAGE_NAME);
	fs::write(&path, NATIVE_SIGNED).unwrap();
	let tab = ImageTab::open(File::open(path).unwrap()).unwrap();
	assert!(tab.report.container.embedded_signature.is_some());
	let check = tab.snapshot.verify(&Cancellation::default());
	assert!(check.passed());
	assert_eq!(check.coverage.payload, CheckStatus::Passed);
	assert_eq!(check.coverage.security, CheckStatus::NotChecked);
}

#[test]
fn verifies_native_modern_images_only_with_the_selected_decoder() {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().join(IMAGE_NAME);
	for (bytes, decoder_enabled) in [
		(NATIVE_ULFO, cfg!(feature = "lzfse")),
		(NATIVE_ULMO, cfg!(feature = "lzma")),
	] {
		fs::write(&path, bytes).unwrap();
		let tab = ImageTab::open(File::open(&path).unwrap()).unwrap();
		let runs = tab.report.container.runs;
		assert!(runs.lzfse + runs.lzma2 > 0);
		assert_eq!(runs.decode_unavailable == 0, decoder_enabled);
		let check = tab.snapshot.verify(&Cancellation::default());
		let expected = if decoder_enabled {
			CheckStatus::Passed
		} else {
			CheckStatus::Unavailable
		};
		assert_eq!(check.coverage.payload, expected);
		assert_eq!(check.passed(), decoder_enabled);
		assert_eq!(fs::read(&path).unwrap(), bytes);
	}
}

#[test]
fn changed_image_never_reuses_the_opened_index_or_reports_passed() {
	let scratch = tempfile::tempdir().unwrap();
	let path = scratch.path().join(IMAGE_NAME);
	fs::write(&path, NATIVE_UDZO).unwrap();
	let tab = ImageTab::open(File::open(&path).unwrap()).unwrap();
	fs::OpenOptions::new()
		.write(true)
		.open(&path)
		.unwrap()
		.set_len(0)
		.unwrap();
	assert!(matches!(
		tab.snapshot.run_page(0),
		Err(Error::SourceChanged)
	));
	assert!(matches!(
		tab.snapshot.locate_run(0),
		Err(Error::SourceChanged)
	));
	let result = tab.snapshot.verify(&Cancellation::default());
	assert_eq!(result.outcome, VerificationOutcome::Failed);
	assert_eq!(
		result.diagnostic.unwrap().category,
		ErrorCategory::SourceChanged
	);
}
