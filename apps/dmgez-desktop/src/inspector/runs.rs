use dmgez_inspector::{Run, RunPage};
#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
use dmgez_runtime::image::RunCoverage;
use dmgez_runtime::image::{RunFilter, RunOrder};
use eframe::egui::{self, Align, Grid, Key, TextEdit, Ui};

use crate::image::{
	RUN_PAGE_SIZE, RunNavigation,
	exports::{Kind as Export, displayed_run},
};

use super::Action;

const MAX_OFFSET_TEXT: usize = u64::MAX.ilog10() as usize + 1;
const HEX_RADIX: u32 = 16;
const FILTERS: [(RunFilter, &str); 8] = [
	(RunFilter::All, "All codecs"),
	(RunFilter::Raw, "Raw"),
	(RunFilter::Zero, "Zero"),
	(RunFilter::Ignore, "Ignore"),
	(RunFilter::Zlib, "Zlib"),
	(RunFilter::Lzfse, "LZFSE"),
	(RunFilter::Lzma2, "LZMA2"),
	(RunFilter::DecoderUnavailable, "Decoder unavailable"),
];
const ORDERS: [(RunOrder, &str); 4] = [
	(RunOrder::Logical, "Logical offset"),
	(RunOrder::LargestLogical, "Largest logical size"),
	(RunOrder::LargestStored, "Largest stored size"),
	(RunOrder::StoredOffset, "Stored offset"),
];

pub(super) fn show(
	ui: &mut Ui,
	page: &RunPage,
	total: usize,
	navigation: &mut RunNavigation,
	idle: bool,
) -> Option<Action> {
	ui.separator();
	#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
	if navigation.file_runs.is_some() {
		return show_coverage(ui, page, navigation, idle);
	}
	ui.heading("Runs");
	let locate = show_run_lookup(ui, navigation, idle);
	let query = query_controls(ui, navigation, idle);
	let export = show_export(ui, page, navigation, idle);
	let reveal = std::mem::take(&mut navigation.reveal_selected);
	let active = navigation.query_page.as_ref();
	let selected = navigation.selected;
	let (page_action, clicked) = if let Some(active) = active {
		let paging = pager(
			ui,
			active.offset,
			active.rows.len(),
			active.next_offset,
			active.total_matches,
			idle,
			|offset| Action::QueryRuns {
				query: active.query,
				offset,
			},
		);
		let rows = active.rows.iter().map(|row| (row.ordinal, &row.run));
		(paging, rows_grid(ui, rows, selected, reveal, idle))
	} else {
		let offset = usize::try_from(page.offset).ok()?;
		let paging = pager(
			ui,
			offset,
			page.runs.len(),
			page.next_offset,
			total,
			idle,
			Action::Page,
		);
		let rows = (offset..).zip(&page.runs);
		(paging, rows_grid(ui, rows, selected, reveal, idle))
	};
	if let Some(clicked) = clicked {
		navigation.select_displayed(Some(clicked));
	}
	locate.or(query).or(page_action).or(export)
}

#[cfg(any(feature = "hfsplus-read", feature = "apfs-read"))]
fn show_coverage(
	ui: &mut Ui,
	page: &RunPage,
	navigation: &mut RunNavigation,
	idle: bool,
) -> Option<Action> {
	let locate = show_run_lookup(ui, navigation, idle);
	let reveal = std::mem::take(&mut navigation.reveal_selected);
	let file = navigation.file_runs.as_ref()?;
	let heading = ui.heading(format!(
		"Runs covering {}",
		crate::image::browse::label(&file.name)
	));
	if reveal && navigation.selected.is_none() {
		heading.scroll_to_me(Some(Align::Min));
	}
	let all = ui
		.add_enabled(idle, egui::Button::new("Show all runs"))
		.clicked()
		.then_some(Action::Page(0));
	let export = show_export(ui, page, navigation, idle);
	match &file.coverage {
		RunCoverage::Mapped(report) if report.rows.is_empty() => {
			ui.label("No stored disk bytes cover this file's data.");
		}
		RunCoverage::Mapped(report) => {
			if report.rows.len() < report.total {
				ui.label(format!(
					"Showing the first {} of {} covering runs.",
					report.rows.len(),
					report.total
				));
			}
			let rows = report.rows.iter().map(|row| (row.ordinal, &row.run));
			if let Some(ordinal) = rows_grid(ui, rows, navigation.selected, reveal, idle) {
				navigation.select_displayed(Some(ordinal));
			}
		}
		RunCoverage::Limit => {
			ui.label("The file exceeds the coverage observation limit.");
		}
		_ => {
			ui.label("Covering runs are unavailable for this storage profile.");
		}
	}
	locate.or(all).or(export)
}

fn query_controls(ui: &mut Ui, navigation: &mut RunNavigation, idle: bool) -> Option<Action> {
	let active = navigation.query_page.as_ref().map(|page| page.query);
	let action = ui
		.add_enabled_ui(idle, |ui| {
			ui.horizontal_wrapped(|ui| {
				show_options(ui, "Filter runs", &mut navigation.query.filter, &FILTERS);
				show_options(ui, "Sort runs", &mut navigation.query.order, &ORDERS);
				let apply = ui
					.button("Apply run query")
					.clicked()
					.then_some(Action::QueryRuns {
						query: navigation.query,
						offset: 0,
					});
				let reset = ui
					.add_enabled(active.is_some(), egui::Button::new("Clear run query"))
					.clicked()
					.then_some(Action::Page(0));
				apply.or(reset)
			})
			.inner
		})
		.inner;
	if let Some(active) = active {
		ui.label(format!(
			"{} · {}",
			title(active.filter, &FILTERS),
			title(active.order, &ORDERS)
		));
	}
	action
}

fn show_options<T: Copy + PartialEq>(
	ui: &mut Ui,
	label: &str,
	value: &mut T,
	options: &[(T, &str)],
) {
	egui::ComboBox::from_label(label)
		.selected_text(title(*value, options))
		.show_ui(ui, |ui| {
			for &(choice, title) in options {
				ui.selectable_value(value, choice, title);
			}
		});
}

fn title<'label, T: PartialEq>(value: T, options: &[(T, &'label str)]) -> &'label str {
	options
		.iter()
		.find_map(|(choice, title)| (*choice == value).then_some(*title))
		.unwrap_or_default()
}

fn show_export(
	ui: &mut Ui,
	page: &RunPage,
	navigation: &RunNavigation,
	idle: bool,
) -> Option<Action> {
	let selected = navigation.selected?;
	ui.horizontal_wrapped(|ui| {
		ui.label(format!("Selected run {}", selected + 1));
		let exportable = displayed_run(page, navigation);
		if ui
			.add_enabled(
				idle && exportable.is_some(),
				egui::Button::new("Export selected run…"),
			)
			.clicked()
		{
			exportable.map(|ordinal| Action::Export(Export::Run(ordinal)))
		} else {
			None
		}
	})
	.inner
}

fn pager(
	ui: &mut Ui,
	offset: usize,
	rows: usize,
	next_offset: Option<usize>,
	total: usize,
	idle: bool,
	action_at: impl Fn(usize) -> Action,
) -> Option<Action> {
	if rows == 0 {
		ui.label("No runs match this view.");
		return None;
	}
	ui.horizontal_wrapped(|ui| {
		ui.label(format!("{}–{} of {total}", offset + 1, offset + rows));
		let current = offset / RUN_PAGE_SIZE + 1;
		let mut requested = current;
		let previous = ui
			.add_enabled(idle && offset > 0, egui::Button::new("Previous page"))
			.clicked();
		let next = ui
			.add_enabled(
				idle && next_offset.is_some(),
				egui::Button::new("Next page"),
			)
			.clicked();
		let pages = total.div_ceil(RUN_PAGE_SIZE);
		if pages > 1 {
			ui.add_enabled(
				idle,
				egui::DragValue::new(&mut requested)
					.range(1..=pages)
					.prefix("Page ")
					.update_while_editing(false),
			);
		}
		if previous {
			Some(action_at(offset.saturating_sub(RUN_PAGE_SIZE)))
		} else if next {
			next_offset.map(action_at)
		} else if requested != current {
			Some(action_at((requested - 1) * RUN_PAGE_SIZE))
		} else {
			None
		}
	})
	.inner
}

fn rows_grid<'a>(
	ui: &mut Ui,
	rows: impl Iterator<Item = (usize, &'a Run)>,
	selected: Option<usize>,
	reveal: bool,
	idle: bool,
) -> Option<usize> {
	Grid::new("run_rows")
		.striped(true)
		.show(ui, |ui| {
			for heading in [
				"Run",
				"Codec",
				"Logical start",
				"Logical bytes",
				"Stored start",
				"Stored bytes",
			] {
				ui.strong(heading);
			}
			ui.end_row();
			let mut target = None;
			for (ordinal, run) in rows {
				let number = ordinal + 1;
				let response = ui.add_enabled(
					idle,
					egui::Button::selectable(selected == Some(ordinal), number.to_string()),
				);
				response.widget_info(|| {
					egui::WidgetInfo::selected(
						egui::Role::Button,
						response.enabled(),
						selected == Some(ordinal),
						format!("Select run {number}"),
					)
				});
				if response.clicked() {
					target = Some(ordinal);
				}
				if selected == Some(ordinal) && reveal {
					response.scroll_to_me(Some(Align::Center));
				}
				ui.label(format!("{:?}", run.encoding));
				ui.label(run.logical.offset_bytes.to_string());
				ui.label(run.logical.length_bytes.to_string());
				if run.physical.length_bytes == 0 {
					ui.label("—");
				} else {
					ui.label(run.physical.offset_bytes.to_string());
				}
				ui.label(run.physical.length_bytes.to_string());
				ui.end_row();
			}
			target
		})
		.inner
}

fn show_run_lookup(ui: &mut Ui, navigation: &mut RunNavigation, idle: bool) -> Option<Action> {
	let requested = ui
		.horizontal_wrapped(|ui| {
			let label = ui.label("Logical byte offset");
			let input = ui
				.add_enabled(
					idle,
					TextEdit::singleline(&mut navigation.offset)
						.hint_text("Decimal or 0x…")
						.desired_width(180.0)
						.char_limit(MAX_OFFSET_TEXT + 1),
				)
				.labelled_by(label.id);
			if input.changed() {
				navigation.invalid_offset = false;
			}
			let clicked = ui
				.add_enabled(idle, egui::Button::new("Go to run"))
				.on_hover_text("Open the corresponding page in logical order")
				.clicked();
			clicked
				|| (idle && input.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
		})
		.inner;
	let action = if requested {
		let offset = parse_logical_offset(&navigation.offset);
		navigation.invalid_offset = offset.is_none();
		offset.map(Action::LocateRun)
	} else {
		None
	};
	if navigation.invalid_offset {
		ui.label("Enter a decimal byte offset or a hexadecimal offset starting with 0x.");
	}
	action
}

fn parse_logical_offset(input: &str) -> Option<u64> {
	// Keep a character beyond the representable input so truncated paste is rejected.
	if input.len() > MAX_OFFSET_TEXT {
		return None;
	}
	let value = input.trim();
	if let Some(hex) = value
		.strip_prefix('0')
		.and_then(|rest| rest.strip_prefix(['x', 'X']))
	{
		u64::from_str_radix(hex, HEX_RADIX).ok()
	} else {
		value.parse().ok()
	}
}
