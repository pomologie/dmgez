use std::collections::TryReserveError;

use dmgez_config::Arrangement;
use dmgez_preview::{CanvasPoint, GeometryError, PlacedItem, PositionSource, ProjectFolder};
use thiserror::Error;

use crate::report::{Diagnostic, Evidence, Icon, Report, RuleId, SCHEMA_VERSION};

const DEFAULT_MAX_ITEMS: usize = 100_000;
const DEFAULT_MAX_PAIR_CHECKS: usize = 1_000_000;
const DEFAULT_MAX_DIAGNOSTICS: usize = 10_000;
// Design guidance for Finder grid placement, in logical points.
const RECOMMENDED_MINIMUM_GRID_SPACING: f64 = 100.0;

/// A rule-wide suppression supplied by the caller, with an exact reason.
#[derive(Clone, Copy, Debug)]
pub struct Suppression<'input> {
	rule: RuleId,
	reason: &'input str,
}

impl<'input> Suppression<'input> {
	/// Creates a suppression with a caller-supplied reason.
	/// # Errors
	/// Returns an error for an empty or whitespace-only reason.
	pub fn new(rule: RuleId, reason: &'input str) -> Result<Self, Error> {
		if reason.trim().is_empty() {
			return Err(Error::SuppressionReason(rule));
		}
		Ok(Self { rule, reason })
	}
}

/// Bounds for one lint operation over an already resolved folder.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	pub max_items: usize,
	/// Maximum candidate pairs checked after pruning disjoint horizontal spans.
	pub max_pair_checks: usize,
	/// Maximum findings retained, including suppressed findings.
	pub max_diagnostics: usize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_items: DEFAULT_MAX_ITEMS,
			max_pair_checks: DEFAULT_MAX_PAIR_CHECKS,
			max_diagnostics: DEFAULT_MAX_DIAGNOSTICS,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Budget {
	Items,
	PairChecks,
	Diagnostics,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
	#[error(transparent)]
	Geometry(#[from] GeometryError),
	#[error("layout lint reached its {budget:?} limit of {limit}")]
	Limit { budget: Budget, limit: usize },
	#[error("could not reserve a bounded layout lint buffer")]
	Allocation(#[from] TryReserveError),
	#[error("layout lint was cancelled")]
	Cancelled,
	#[error("suppression for {0:?} requires a reason")]
	SuppressionReason(RuleId),
	#[error("layout lint has multiple suppressions for {0:?}")]
	DuplicateSuppression(RuleId),
}

/// Lints the configured positions in the selected preview's initial viewport.
/// The overlap check is limited to configured, visible icons with positive-area
/// intersections inside that viewport. Hidden configured icons are diagnosed
/// separately. Preview grid placements stand in for entries without explicit
/// positions and contribute only to whether grid spacing is relevant.
///
/// Findings describe icon rectangles in the shared design model. Automatic
/// arrangement can override configured positions when Finder displays the image.
/// # Errors
/// Returns a geometry, budget, allocation, suppression or cancellation error
/// without producing a partial report.
pub fn lint_layout<'input>(
	folder: &ProjectFolder<'input>,
	limits: Limits,
	suppressions: &[Suppression<'input>],
	mut cancelled: impl FnMut() -> bool,
) -> Result<Report<'input>, Error> {
	check_cancellation(&mut cancelled)?;
	check_limit(
		folder.items().len() > limits.max_items,
		Budget::Items,
		limits.max_items,
	)?;
	for (index, suppression) in suppressions.iter().enumerate() {
		if suppressions[..index]
			.iter()
			.any(|earlier| earlier.rule == suppression.rule)
		{
			return Err(Error::DuplicateSuppression(suppression.rule));
		}
	}

	let style = folder.style();
	let viewport = folder.visible_bounds(style.viewport(CanvasPoint::new(0.0, 0.0)?, 1.0)?)?;
	let mut findings = Findings {
		diagnostics: Vec::new(),
		suppressions,
		limit: limits.max_diagnostics,
	};
	let mut in_view = Vec::new();
	let mut configured_visible = 0;
	let mut grid_placement = false;
	for item in folder.items() {
		check_cancellation(&mut cancelled)?;
		if item.position_source == PositionSource::PreviewGrid {
			grid_placement |= !item.hidden;
			continue;
		}
		if item.hidden {
			findings.add(Evidence::HiddenPositionedIcon {
				destination: item.destination.as_str(),
			})?;
			continue;
		}
		configured_visible += 1;
		if !viewport.intersects(item.icon_bounds) {
			findings.add(Evidence::IconOutsideViewport { icon: icon(item) })?;
			continue;
		}
		if !viewport.contains_rect(item.icon_bounds) {
			findings.add(Evidence::IconClipped { icon: icon(item) })?;
		}
		in_view.try_reserve(1)?;
		in_view.push(item);
	}
	if style.arrangement != Arrangement::None && configured_visible != 0 {
		findings.add(Evidence::AutomaticArrangement {
			configured_visible_icons: configured_visible,
		})?;
		grid_placement = true;
	}
	if grid_placement && style.grid_spacing < RECOMMENDED_MINIMUM_GRID_SPACING {
		findings.add(Evidence::TightGridSpacing {
			spacing_points: style.grid_spacing,
			recommended_minimum_points: RECOMMENDED_MINIMUM_GRID_SPACING,
		})?;
	}
	check_overlaps(
		&mut in_view,
		limits.max_pair_checks,
		&mut findings,
		&mut cancelled,
	)?;
	check_cancellation(&mut cancelled)?;
	Ok(Report {
		schema_version: SCHEMA_VERSION,
		viewport: viewport.into(),
		diagnostics: findings.diagnostics,
	})
}

fn check_overlaps<'input>(
	items: &mut [&PlacedItem<'input>],
	limit: usize,
	findings: &mut Findings<'_, 'input>,
	cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
	items.sort_unstable_by(|left, right| {
		left.icon_bounds
			.min()
			.x()
			.total_cmp(&right.icon_bounds.min().x())
			.then_with(|| left.destination.cmp(right.destination))
	});
	let mut remaining = limit;
	for (index, first) in items.iter().enumerate() {
		check_cancellation(cancelled)?;
		for second in &items[index + 1..] {
			if second.icon_bounds.min().x() >= first.icon_bounds.max().x() {
				break;
			}
			check_cancellation(cancelled)?;
			check_limit(remaining == 0, Budget::PairChecks, limit)?;
			remaining -= 1;
			// Both icons intersect the viewport. For axis-aligned rectangles,
			// pairwise positive intersections imply a positive common intersection.
			if first.icon_bounds.intersects(second.icon_bounds) {
				let (first, second) = if first.destination < second.destination {
					(first, second)
				} else {
					(second, first)
				};
				findings.add(Evidence::IconsOverlap {
					first: icon(first),
					second: icon(second),
				})?;
			}
		}
	}
	Ok(())
}

fn icon<'input>(item: &PlacedItem<'input>) -> Icon<'input> {
	Icon {
		destination: item.destination.as_str(),
		bounds: item.icon_bounds.into(),
	}
}

struct Findings<'policy, 'input> {
	diagnostics: Vec<Diagnostic<'input>>,
	suppressions: &'policy [Suppression<'input>],
	limit: usize,
}

impl<'input> Findings<'_, 'input> {
	fn add(&mut self, evidence: Evidence<'input>) -> Result<(), Error> {
		check_limit(
			self.diagnostics.len() == self.limit,
			Budget::Diagnostics,
			self.limit,
		)?;
		let rule = evidence.rule();
		let suppression_reason = self
			.suppressions
			.iter()
			.find(|item| item.rule == rule)
			.map(|item| item.reason);
		self.diagnostics.try_reserve(1)?;
		self.diagnostics.push(Diagnostic {
			rule,
			severity: rule.severity(),
			evidence,
			suppression_reason,
		});
		Ok(())
	}
}

fn check_cancellation(cancelled: &mut impl FnMut() -> bool) -> Result<(), Error> {
	if cancelled() {
		Err(Error::Cancelled)
	} else {
		Ok(())
	}
}

fn check_limit(reached: bool, budget: Budget, limit: usize) -> Result<(), Error> {
	if reached {
		Err(Error::Limit { budget, limit })
	} else {
		Ok(())
	}
}
