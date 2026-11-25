//! Configured icon geometry and suppression coverage for D-REL-04-T01.

use dmgez_config::{Project, VirtualPath};
use dmgez_lint::{
	Bounds, Budget, Error, Evidence, Limits, RuleId, Severity, Suppression, lint_layout,
};
use dmgez_preview::{ItemInput, ProjectFolder};

const ROOT: &str = "/";
const NESTED_FOLDER: &str = "/Docs";
const IMAGE: &str = "schema_version=1\n[image]\nname='Layout'\noutput='layout.dmg'\n";
const GEOMETRY: &str = r#"
[[contents]]
type = "directory"
destination = "/Alpha"
position = [25, 35]

[[contents]]
type = "directory"
destination = "/Beta"

[[contents]]
type = "directory"
destination = "/Clip"
position = [105, 45]

[[contents]]
type = "directory"
destination = "/Hidden"
position = [25, 35]
hidden = true

[[contents]]
type = "directory"
destination = "/Outside"
position = [120, 45]

[[contents]]
type = "directory"
destination = "/Touch"
position = [53, 35]

[[contents]]
type = "directory"
destination = "/Unpositioned"

[finder]
window_size = [100, 80]

[finder.icon_view]
icon_size = 20
grid_spacing = 100
scroll_position = [10, 20]
positions = { "Beta" = [33, 35] }
"#;

fn project(settings: &str) -> Project {
	Project::parse(&format!("{IMAGE}{settings}")).unwrap()
}

fn root(project: &Project) -> ProjectFolder<'_> {
	let folder: VirtualPath = ROOT.parse().unwrap();
	let children: Vec<_> = project.contents.iter().map(ItemInput::from).collect();
	ProjectFolder::from_project(project, &folder, &children).unwrap()
}

#[test]
fn checks_scrolled_viewport_and_only_overlapping_visible_configured_icons() {
	let project = project(GEOMETRY);
	let folder = root(&project);
	let report = lint_layout(&folder, Limits::default(), &[], || false).unwrap();
	assert_eq!(
		report.viewport,
		Bounds {
			min: [10.0, 20.0],
			max: [110.0, 100.0]
		}
	);
	let [clipped, hidden, outside, overlap] = report.diagnostics.as_slice() else {
		panic!("expected clipping, hidden icon, outside viewport, and one overlap");
	};
	assert!(
		matches!(clipped.evidence, Evidence::IconClipped { icon } if icon.destination == "/Clip" && icon.bounds.max == [115.0, 55.0])
	);
	assert!(
		matches!(outside.evidence, Evidence::IconOutsideViewport { icon } if icon.destination == "/Outside" && icon.bounds.min == [110.0, 35.0])
	);
	assert_eq!(hidden.severity, Severity::Info);
	assert!(matches!(
		hidden.evidence,
		Evidence::HiddenPositionedIcon {
			destination: "/Hidden"
		}
	));
	assert_eq!(overlap.severity, Severity::Warning);
	assert!(
		matches!(overlap.evidence, Evidence::IconsOverlap { first, second } if first.destination == "/Alpha" && second.destination == "/Beta")
	);
}

#[test]
fn preserves_reason_and_versioned_rule_names_in_serialized_results() {
	const REASON: &str = "  Intentional edge layout  ";
	let project = project(GEOMETRY);
	let folder = root(&project);
	let suppression = Suppression::new(RuleId::IconClipped, REASON).unwrap();
	let report = lint_layout(&folder, Limits::default(), &[suppression], || false).unwrap();
	assert_eq!(report.diagnostics[0].suppression_reason, Some(REASON));
	assert!(report.diagnostics[3].suppression_reason.is_none());
	let json = serde_json::to_value(&report).unwrap();
	assert_eq!(json["schema_version"], dmgez_lint::SCHEMA_VERSION);
	assert_eq!(json["diagnostics"][0]["suppression_reason"], REASON);
	for (expected, observed) in report
		.diagnostics
		.iter()
		.zip(json["diagnostics"].as_array().unwrap())
	{
		assert_eq!(observed["rule"], expected.rule.as_str());
		assert_eq!(observed["evidence"]["kind"], expected.rule.as_str());
	}
	assert!(matches!(
		Suppression::new(RuleId::IconClipped, "  \n"),
		Err(Error::SuppressionReason(RuleId::IconClipped))
	));
	assert!(matches!(
		lint_layout(
			&folder,
			Limits::default(),
			&[suppression, suppression],
			|| false
		),
		Err(Error::DuplicateSuppression(RuleId::IconClipped))
	));
}

#[test]
fn reports_scope_local_arrangement_and_tight_grid_spacing() {
	let project = project(
		r#"
[[contents]]
type = "directory"
destination = "/Docs/Manual"
position = [30, 30]

[finder]
window_size = [100, 80]

[finder.icon_view]
icon_size = 20
grid_spacing = 100

[[finder.folders]]
path = "/Docs"
window_size = [70, 60]

[finder.folders.icon_view]
arrange_by = "name"
grid_spacing = 75
scroll_position = [5, 9]
"#,
	);
	let folder_path = NESTED_FOLDER.parse().unwrap();
	let children: Vec<_> = project.contents.iter().map(ItemInput::from).collect();
	let folder = ProjectFolder::from_project(&project, &folder_path, &children).unwrap();
	let report = lint_layout(&folder, Limits::default(), &[], || false).unwrap();
	assert_eq!(
		report.viewport,
		Bounds {
			min: [5.0, 9.0],
			max: [75.0, 69.0]
		}
	);
	assert_eq!(
		report
			.diagnostics
			.iter()
			.map(|diagnostic| diagnostic.evidence)
			.collect::<Vec<_>>(),
		[
			Evidence::AutomaticArrangement {
				configured_visible_icons: 1
			},
			Evidence::TightGridSpacing {
				spacing_points: 75.0,
				recommended_minimum_points: 100.0
			},
		]
	);
}

#[test]
fn fails_closed_on_each_budget_and_on_cancellation() {
	let project = project(GEOMETRY);
	let folder = root(&project);
	for (budget, limits) in [
		(
			Budget::Items,
			Limits {
				max_items: 0,
				..Limits::default()
			},
		),
		(
			Budget::PairChecks,
			Limits {
				max_pair_checks: 0,
				..Limits::default()
			},
		),
		(
			Budget::Diagnostics,
			Limits {
				max_diagnostics: 0,
				..Limits::default()
			},
		),
	] {
		assert!(
			matches!(lint_layout(&folder, limits, &[], || false), Err(Error::Limit { budget: observed, limit: 0 }) if observed == budget)
		);
	}
	let mut checks = 0;
	let result = lint_layout(&folder, Limits::default(), &[], || {
		checks += 1;
		checks == 3
	});
	assert!(matches!(result, Err(Error::Cancelled)));
}
