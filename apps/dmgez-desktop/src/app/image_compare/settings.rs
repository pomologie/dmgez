#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use std::{num::NonZeroU32, str::FromStr};

use dmgez_runtime::compare::ByteSpace;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use eframe::egui::{CollapsingHeader, TextEdit};
use eframe::egui::{ComboBox, Ui};

use crate::image::compare::Request;
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use crate::image::compare::Selection;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
	#[default]
	Logical,
	Physical,
	#[cfg(feature = "hfsplus-read")]
	HfsPlus,
	#[cfg(feature = "apfs-read")]
	Apfs,
}

const MODES: &[Mode] = &[
	Mode::Logical,
	Mode::Physical,
	#[cfg(feature = "hfsplus-read")]
	Mode::HfsPlus,
	#[cfg(feature = "apfs-read")]
	Mode::Apfs,
];
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
const DECIMAL_DIGITS_U32: usize = u32::MAX.ilog10() as usize + 1;
#[cfg(feature = "apfs-read")]
const DECIMAL_DIGITS_U64: usize = u64::MAX.ilog10() as usize + 1;

#[derive(Default)]
pub(super) struct Settings {
	mode: Mode,
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	left: Selector,
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	right: Selector,
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	options: dmgez_diff::semantic::Options,
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
#[derive(Default)]
struct Selector {
	partition: String,
	#[cfg(feature = "apfs-read")]
	volume: String,
	#[cfg(feature = "apfs-read")]
	checkpoint: String,
}

impl Settings {
	pub(super) fn show(&mut self, ui: &mut Ui) -> Option<Request> {
		ui.horizontal_wrapped(|ui| {
			let label = ui.label("Comparison");
			ComboBox::from_id_salt("compare_mode")
				.selected_text(self.mode.label())
				.show_ui(ui, |ui| {
					for &mode in MODES {
						ui.selectable_value(&mut self.mode, mode, mode.label());
					}
				})
				.response
				.labelled_by(label.id);
		});
		#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
		if self.mode.semantic() {
			self.semantic_settings(ui);
		}
		let request = self.request();
		if request.is_none() {
			ui.label(
				"Enter positive decimal selectors, or leave them blank for automatic selection.",
			);
		}
		if ui
			.add_enabled(
				request.is_some(),
				eframe::egui::Button::new("Choose other image…"),
			)
			.clicked()
		{
			request
		} else {
			None
		}
	}

	fn request(&self) -> Option<Request> {
		match self.mode {
			Mode::Logical => Some(Request::logical()),
			Mode::Physical => Some(Request::Bytes(ByteSpace::Physical)),
			#[cfg(feature = "hfsplus-read")]
			Mode::HfsPlus => Some(Request::HfsPlus {
				left: self.left.selection(self.mode)?,
				right: self.right.selection(self.mode)?,
				options: self.options,
			}),
			#[cfg(feature = "apfs-read")]
			Mode::Apfs => Some(Request::Apfs {
				left: self.left.selection(self.mode)?,
				right: self.right.selection(self.mode)?,
				options: self.options,
			}),
		}
	}

	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	fn semantic_settings(&mut self, ui: &mut Ui) {
		CollapsingHeader::new("Disk and metadata selection").show(ui, |ui| {
			ui.columns(2, |sides| {
				let [left, right] = sides else {
					return;
				};
				self.left.show(left, self.mode, "Open image");
				self.right.show(right, self.mode, "Other image");
			});
			ui.checkbox(&mut self.options.xattrs, "Compare extended attributes");
			ui.checkbox(&mut self.options.finder_info, "Compare Finder information");
		});
	}
}

impl Mode {
	fn label(self) -> &'static str {
		match self {
			Self::Logical => "Logical disk bytes",
			Self::Physical => "Stored image bytes",
			#[cfg(feature = "hfsplus-read")]
			Self::HfsPlus => "HFS+ or HFSX files",
			#[cfg(feature = "apfs-read")]
			Self::Apfs => "APFS files",
		}
	}

	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	fn semantic(self) -> bool {
		!matches!(self, Self::Logical | Self::Physical)
	}
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
impl Selector {
	fn selection(&self, _mode: Mode) -> Option<Selection> {
		#[cfg(feature = "apfs-read")]
		let (volume, checkpoint) = if _mode == Mode::Apfs {
			use std::num::NonZeroU64;

			let volume =
				parse::<NonZeroU64>(&self.volume)?.map(|id| dmgez_apfs::ObjectId(id.get()));
			let checkpoint = parse::<NonZeroU64>(&self.checkpoint)?
				.map(|id| dmgez_apfs::TransactionId(id.get()));
			(volume, checkpoint)
		} else {
			(None, None)
		};
		Some(Selection {
			partition: parse::<NonZeroU32>(&self.partition)?,
			#[cfg(feature = "apfs-read")]
			volume,
			#[cfg(feature = "apfs-read")]
			checkpoint,
		})
	}

	fn show(&mut self, ui: &mut Ui, _mode: Mode, title: &str) {
		ui.strong(title);
		input(
			ui,
			"Partition slot",
			&mut self.partition,
			DECIMAL_DIGITS_U32,
		);
		#[cfg(feature = "apfs-read")]
		if _mode == Mode::Apfs {
			input(ui, "Volume object ID", &mut self.volume, DECIMAL_DIGITS_U64);
			input(
				ui,
				"Checkpoint XID",
				&mut self.checkpoint,
				DECIMAL_DIGITS_U64,
			);
		}
	}
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
fn input(ui: &mut Ui, label: &str, value: &mut String, max_digits: usize) {
	let label = ui.label(label);
	ui.add(
		TextEdit::singleline(value)
			.hint_text("Automatic")
			.char_limit(max_digits)
			.desired_width(120.0),
	)
	.labelled_by(label.id);
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
fn parse<T: FromStr>(text: &str) -> Option<Option<T>> {
	let text = text.trim();
	if text.is_empty() {
		return Some(None);
	}
	if !text.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	text.parse().ok().map(Some)
}
