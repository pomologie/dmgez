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
fn assistive_actions_reach_the_run_pager_and_container_check() {
	let tab = multi_page_image();
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
			assert!(inspector::show(ui, &tab, true).is_none())
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
			if let Some(emitted) = inspector::show(ui, &tab, true) {
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
	let result = tab.snapshot.verify(&Cancellation::default());
	assert_eq!(result.outcome, VerificationOutcome::Failed);
	assert_eq!(
		result.diagnostic.unwrap().category,
		ErrorCategory::SourceChanged
	);
}
