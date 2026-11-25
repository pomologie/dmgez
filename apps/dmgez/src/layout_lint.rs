use std::collections::BTreeSet;

use dmgez_config::{VirtualPath, project_edit::RevisionId};
use dmgez_lint::{Evidence, Limits, Severity, lint_layout};
use dmgez_preview::ProjectFolder;
use eframe::egui::{Label, ScrollArea, TextStyle, Ui};

const MAX_PAIR_CHECKS: usize = 32_768;
const MAX_DIAGNOSTICS: usize = 512;
const FINDINGS_HEIGHT: f32 = 112.0;

#[derive(Default)]
pub(crate) struct LayoutLint {
	enabled: bool,
	cached: Option<FolderFindings>,
}

struct FolderFindings {
	revision: RevisionId,
	folder: VirtualPath,
	outcome: Result<Findings, String>,
}

struct Findings {
	rows: Vec<String>,
	warnings: BTreeSet<String>,
}

impl LayoutLint {
	pub(crate) fn toggle(&mut self, ui: &mut Ui) {
		ui.checkbox(&mut self.enabled, "Show layout findings");
	}

	pub(crate) fn show(
		&mut self,
		ui: &mut Ui,
		revision: RevisionId,
		folder: &VirtualPath,
		layout: Option<&ProjectFolder<'_>>,
	) -> Option<&BTreeSet<String>> {
		let outcome = self.for_folder(revision, folder, || {
			let layout = layout.ok_or("The selected folder is no longer in the project.")?;
			Findings::scan(layout)
		})?;
		match outcome {
			Ok(findings) => {
				findings.show(ui);
				Some(&findings.warnings)
			}
			Err(error) => {
				ui.label(format!("Could not check the layout. {error}"));
				None
			}
		}
	}

	fn for_folder(
		&mut self,
		revision: RevisionId,
		folder: &VirtualPath,
		scan: impl FnOnce() -> Result<Findings, String>,
	) -> Option<&Result<Findings, String>> {
		if !self.enabled {
			return None;
		}
		if self
			.cached
			.as_ref()
			.is_none_or(|entry| entry.revision != revision || entry.folder != *folder)
		{
			self.cached = Some(FolderFindings {
				revision,
				folder: folder.clone(),
				outcome: scan(),
			});
		}
		self.cached.as_ref().map(|entry| &entry.outcome)
	}
}

impl Findings {
	fn scan(layout: &ProjectFolder<'_>) -> Result<Self, String> {
		let limits = Limits {
			max_pair_checks: MAX_PAIR_CHECKS,
			max_diagnostics: MAX_DIAGNOSTICS,
			..Limits::default()
		};
		let report =
			lint_layout(layout, limits, &[], || false).map_err(|error| error.to_string())?;
		let mut rows = Vec::with_capacity(report.diagnostics.len());
		let mut warnings = BTreeSet::new();
		for diagnostic in report.diagnostics {
			let detail = match diagnostic.evidence {
				Evidence::IconClipped { icon } => {
					warnings.insert(icon.destination.to_owned());
					format!("The window clips {:?}.", icon.destination)
				}
				Evidence::IconOutsideViewport { icon } => {
					warnings.insert(icon.destination.to_owned());
					format!("{:?} is outside the window.", icon.destination)
				}
				Evidence::IconsOverlap { first, second } => {
					warnings.insert(first.destination.to_owned());
					warnings.insert(second.destination.to_owned());
					format!(
						"{:?} and {:?} overlap.",
						first.destination, second.destination
					)
				}
				Evidence::HiddenPositionedIcon { destination } => {
					format!("{:?} has a position and is hidden.", destination)
				}
				Evidence::AutomaticArrangement {
					configured_visible_icons,
				} => format!("Finder may rearrange {configured_visible_icons} positioned icons."),
				Evidence::TightGridSpacing {
					spacing_points,
					recommended_minimum_points,
				} => format!(
					"Grid spacing is {spacing_points} pt. The recommended minimum is {recommended_minimum_points} pt."
				),
				_ => format!("Rule {} reported a finding.", diagnostic.rule.as_str()),
			};
			let level = match diagnostic.severity {
				Severity::Warning => "Warning",
				Severity::Info => "Info",
				_ => "Finding",
			};
			rows.push(format!("{level}. {detail}"));
		}
		Ok(Self { rows, warnings })
	}

	fn show(&self, ui: &mut Ui) {
		if self.rows.is_empty() {
			ui.label("No layout findings in this folder.");
			return;
		}
		ui.strong(format!("Layout findings ({})", self.rows.len()));
		let row_height = ui.text_style_height(&TextStyle::Body);
		ScrollArea::both()
			.id_salt("layout_lint_findings")
			.max_height(FINDINGS_HEIGHT)
			.show_rows(ui, row_height, self.rows.len(), |ui, visible| {
				for row in visible {
					ui.add(Label::new(self.rows[row].as_str()).extend());
				}
			});
	}
}

#[cfg(test)]
mod tests {
	use std::cell::Cell;

	use dmgez_config::project_edit::{ProjectDocument, ProjectEdit};
	use dmgez_preview::DeclaredTree;
	use eframe::egui::{Context, Event, Modifiers, PointerButton, RawInput, Shape};

	use super::*;

	const PROJECT: &str = r#"
schema_version = 1
[image]
name = 'Layout'
output = 'layout.dmg'

[[contents]]
source = 'missing'
destination = '/First'
position = [50, 50]

[[contents]]
source = 'missing'
destination = '/Second'
position = [55, 50]

[[contents]]
source = 'missing'
destination = '/Hidden'
position = [50, 50]
hidden = true

[[contents]]
type = 'directory'
destination = '/Docs'

[finder]
window_size = [200, 200]
[finder.icon_view]
icon_size = 32
"#;

	#[test]
	fn shows_item_warnings_and_text_for_hidden_items() {
		let document = ProjectDocument::open(PROJECT).unwrap();
		let project = document.project();
		let root = "/".parse().unwrap();
		let tree = DeclaredTree::from_project(project).unwrap();
		let layout = tree.layout(project, &root).unwrap();
		let findings = Findings::scan(&layout).unwrap();
		assert_eq!(
			findings
				.warnings
				.iter()
				.map(String::as_str)
				.collect::<Vec<_>>(),
			["/First", "/Second"]
		);
		assert!(
			findings
				.rows
				.iter()
				.any(|row| row.contains("\"/First\" and \"/Second\" overlap"))
		);
		assert!(
			findings
				.rows
				.iter()
				.any(|row| row.starts_with("Info. \"/Hidden\""))
		);
	}

	#[test]
	fn scans_only_when_enabled_and_caches_results_per_folder_revision() {
		let mut document = ProjectDocument::open(PROJECT).unwrap();
		let original = document.revision();
		let root = "/".parse().unwrap();
		let docs = "/Docs".parse().unwrap();
		let mut lint = LayoutLint::default();
		let calls = Cell::new(0);
		let scan = || {
			calls.set(calls.get() + 1);
			Err("simulated diagnostic limit".to_owned())
		};
		assert!(lint.for_folder(original, &root, scan).is_none());
		assert_eq!(calls.get(), 0);

		lint.enabled = true;
		assert!(lint.for_folder(original, &root, scan).unwrap().is_err());
		assert!(lint.for_folder(original, &root, scan).unwrap().is_err());
		assert_eq!(calls.get(), 1);

		assert!(lint.for_folder(original, &docs, scan).unwrap().is_err());
		assert_eq!(calls.get(), 2);
		document
			.apply(&[ProjectEdit::SetImageName("Renamed".to_owned())])
			.unwrap();
		let project = document.project();
		let tree = DeclaredTree::from_project(project).unwrap();
		let layout = tree.layout(project, &docs).unwrap();
		let findings = lint.for_folder(document.revision(), &docs, || Findings::scan(&layout));
		assert!(findings.unwrap().as_ref().unwrap().rows.is_empty());
		assert!(
			lint.for_folder(document.revision(), &docs, scan)
				.unwrap()
				.is_ok()
		);
		assert_eq!(calls.get(), 2);

		assert!(document.undo().unwrap());
		assert!(
			lint.for_folder(document.revision(), &docs, scan)
				.unwrap()
				.is_err()
		);
		assert_eq!(calls.get(), 3);
	}

	#[test]
	fn toggle_displays_and_dismisses_text_findings() {
		const TOGGLE_LABEL: &str = "Show layout findings";
		const WARNING: &str = "Warning. \"/First\" and \"/Second\" overlap.";
		let document = ProjectDocument::open(PROJECT).unwrap();
		let project = document.project();
		let folder = "/".parse().unwrap();
		let tree = DeclaredTree::from_project(project).unwrap();
		let layout = tree.layout(project, &folder).unwrap();
		let ctx = Context::default();
		let mut state = LayoutLint::default();
		let mut frame = |events| {
			let mut output = ctx.run_ui(
				RawInput {
					events,
					..Default::default()
				},
				|ui| {
					state.toggle(ui);
					state.show(ui, document.revision(), &folder, Some(&layout));
				},
			);
			output.textures_delta.clear();
			output
		};
		let initial = frame(Vec::new());
		let toggle = initial
			.shapes
			.iter()
			.find_map(|shape| match &shape.shape {
				Shape::Text(text) if text.galley.text() == TOGGLE_LABEL => {
					Some(text.pos + text.galley.size() / 2.0)
				}
				_ => None,
			})
			.expect("the toggle label is painted");
		let click = |pressed| Event::PointerButton {
			pos: toggle,
			button: PointerButton::Primary,
			pressed,
			modifiers: Modifiers::NONE,
		};
		let warning_visible = |output: &eframe::egui::FullOutput| {
			output.shapes.iter().any(
				|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.text() == WARNING),
			)
		};
		assert!(!warning_visible(&initial));
		let _ = frame(vec![Event::PointerMoved(toggle), click(true)]);
		assert!(warning_visible(&frame(vec![click(false)])));
		let _ = frame(vec![click(true)]);
		assert!(!warning_visible(&frame(vec![click(false)])));
	}
}
