use dmgez_diff::semantic::{self, ChangeKind, Field};
use eframe::egui::{CollapsingHeader, Label, Role, ScrollArea, Ui, WidgetInfo};

use crate::image::compare::FilesystemReport;

const CHANGE_LIST_HEIGHT: f32 = 220.0;

pub(super) fn show(ui: &mut Ui, report: &FilesystemReport, selected: &mut Option<usize>) {
	let comparison = &report.comparison;
	ui.columns(2, |sides| {
		sides[0].label(format!(
			"{} · {} paths",
			report.left, comparison.left_entries
		));
		sides[1].label(format!(
			"{} · {} paths",
			report.right, comparison.right_entries
		));
	});
	ui.strong(match comparison.outcome {
		semantic::Outcome::Equal => "No differences in compared fields".into(),
		semantic::Outcome::Different => format!("{} changed paths", comparison.changes.len()),
		_ => "Compared filesystem paths".into(),
	});
	CollapsingHeader::new("Comparison scope").show(ui, |ui| {
		for field in comparison.policy.compared {
			ui.label(field_label(*field));
		}
		ui.label(format!(
			"{} payload bytes compared",
			comparison.compared_bytes
		));
	});
	if comparison.changes.is_empty() {
		return;
	}
	ui.columns(3, |columns| {
		columns[0].strong("Open image");
		columns[1].strong("Other image");
		columns[2].strong("Change");
	});
	let row_height = ui.spacing().interact_size.y;
	ScrollArea::vertical()
		.id_salt("changed_paths")
		.max_height(CHANGE_LIST_HEIGHT)
		.show_rows(ui, row_height, comparison.changes.len(), |ui, rows| {
			for index in rows {
				let change = &comparison.changes[index];
				ui.push_id(index, |ui| {
					ui.columns(3, |columns| {
						path_cell(&mut columns[0], left_path(change));
						path_cell(&mut columns[1], right_path(change));
						let active = *selected == Some(index);
						let kind = kind_label(change.kind);
						let response = columns[2].selectable_label(active, kind);
						response.widget_info(|| {
							let description = format!("{kind} {}", path(&change.path));
							WidgetInfo::selected(
								Role::Button,
								response.enabled(),
								active,
								description,
							)
						});
						if response.clicked() {
							*selected = Some(index);
						}
					});
				});
			}
		});
	if let Some(change) = selected.and_then(|index| comparison.changes.get(index)) {
		ui.separator();
		ui.strong(path(&change.path));
		for &field in &change.fields {
			ui.label(field_label(field));
		}
		for attribute in &change.xattrs {
			ui.label(format!(
				"{} attribute {}",
				kind_label(attribute.kind),
				attribute.name.escape_debug()
			));
		}
	}
}

fn left_path(change: &semantic::Change) -> Option<&[String]> {
	(change.kind != ChangeKind::Added).then_some(change.path.as_slice())
}

fn right_path(change: &semantic::Change) -> Option<&[String]> {
	(change.kind != ChangeKind::Removed)
		.then_some(change.right_path.as_deref().unwrap_or(&change.path))
}

fn path_cell(ui: &mut Ui, components: Option<&[String]>) {
	let label = components.map_or_else(|| "—".into(), path);
	ui.add(Label::new(&label).truncate()).on_hover_text(label);
}

fn path(components: &[String]) -> String {
	let mut result = String::from("/");
	for (index, component) in components.iter().enumerate() {
		if index != 0 {
			result.push('/');
		}
		result.extend(component.escape_debug());
	}
	result
}

fn kind_label(kind: ChangeKind) -> &'static str {
	match kind {
		ChangeKind::Added => "Added",
		ChangeKind::Removed => "Removed",
		ChangeKind::Changed => "Changed",
		_ => "Difference",
	}
}

fn field_label(field: Field) -> &'static str {
	match field {
		Field::Kind => "Object kind",
		Field::Mode => "Mode",
		Field::Uid => "Owner ID",
		Field::Gid => "Group ID",
		Field::HardLinkTopology => "Hard-link topology",
		Field::DataFork => "Data fork",
		Field::ResourceFork => "Resource fork",
		Field::SymlinkTarget => "Symlink target",
		Field::ExtendedAttributes => "Extended attributes",
		Field::FinderInfo => "Finder information",
		_ => field.as_str(),
	}
}
