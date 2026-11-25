use std::{
	io::{self, Write},
	path::Path,
	process::ExitCode,
};

use dmgez_config::{Project, VirtualPath};
use dmgez_lint::{
	Diagnostic, Evidence, Limits, Report, RuleId, Severity, Suppression, lint_layout,
};
use dmgez_preview::DeclaredTree;
use serde::Serialize;

use crate::{CliResult, LAYOUT_PROFILE_NAME, SUPPRESSION_VALUE_NAME, project_input};

const ROOT_FOLDER: &str = "/";
const SUPPRESSION_SEPARATOR: char = '=';
const LINT_FINDINGS_EXIT_CODE: u8 = 1;

#[derive(Serialize)]
struct LayoutReport<'input> {
	profile: &'static str,
	folder: &'input str,
	#[serde(flatten)]
	layout: Report<'input>,
}

pub(crate) fn run(
	path: &Path,
	folder: Option<&str>,
	suppressions: &[String],
	json: bool,
) -> CliResult<ExitCode> {
	let folder: VirtualPath = folder.unwrap_or(ROOT_FOLDER).parse()?;
	let suppressions = suppressions
		.iter()
		.map(|argument| parse_suppression(argument))
		.collect::<CliResult<Vec<_>>>()?;
	let project = Project::parse(&project_input::read(path)?)?;
	let tree = DeclaredTree::from_project(&project)?;
	if !tree.contains_folder(&folder) {
		return Err(format!("no declared project folder at {:?}", folder.as_str()).into());
	}
	let layout = tree.layout(&project, &folder)?;
	let layout = lint_layout(&layout, Limits::default(), &suppressions, || false)?;
	let has_warnings = layout.diagnostics.iter().any(|finding| {
		finding.severity == Severity::Warning && finding.suppression_reason.is_none()
	});
	let report = LayoutReport {
		profile: LAYOUT_PROFILE_NAME,
		folder: folder.as_str(),
		layout,
	};
	let mut stdout = io::stdout().lock();
	if json {
		serde_json::to_writer(&mut stdout, &report)?;
		writeln!(stdout)?;
	} else {
		writeln!(stdout, "profile = {}", report.profile)?;
		writeln!(stdout, "folder = {:?}", report.folder)?;
		writeln!(stdout, "diagnostics = {}", report.layout.diagnostics.len())?;
		for diagnostic in &report.layout.diagnostics {
			write_diagnostic(&mut stdout, diagnostic)?;
		}
	}
	stdout.flush()?;
	Ok(if has_warnings {
		ExitCode::from(LINT_FINDINGS_EXIT_CODE)
	} else {
		ExitCode::SUCCESS
	})
}

fn parse_suppression(argument: &str) -> CliResult<Suppression<'_>> {
	let (rule, reason) = argument
		.split_once(SUPPRESSION_SEPARATOR)
		.ok_or_else(|| format!("a lint suppression requires {SUPPRESSION_VALUE_NAME}"))?;
	Ok(Suppression::new(rule.parse::<RuleId>()?, reason)?)
}

fn write_diagnostic(output: &mut impl Write, diagnostic: &Diagnostic<'_>) -> io::Result<()> {
	write!(
		output,
		"{} {}",
		diagnostic.severity.as_str(),
		diagnostic.rule.as_str()
	)?;
	match diagnostic.evidence {
		Evidence::IconClipped { icon } | Evidence::IconOutsideViewport { icon } => {
			write!(output, " {:?}", icon.destination)?;
		}
		Evidence::IconsOverlap { first, second } => {
			write!(
				output,
				" {:?} and {:?}",
				first.destination, second.destination
			)?;
		}
		Evidence::HiddenPositionedIcon { destination } => write!(output, " {destination:?}")?,
		Evidence::AutomaticArrangement {
			configured_visible_icons,
		} => write!(output, " {configured_visible_icons} positioned icons")?,
		Evidence::TightGridSpacing {
			spacing_points,
			recommended_minimum_points,
		} => write!(
			output,
			" {spacing_points} pt (recommended minimum {recommended_minimum_points} pt)"
		)?,
		_ => {}
	}
	if let Some(reason) = diagnostic.suppression_reason {
		write!(output, " (suppressed {reason:?})")?;
	}
	writeln!(output)
}
