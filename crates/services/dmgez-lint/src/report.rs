use dmgez_preview::FolderRect;
use serde::Serialize;

/// Version of the serialized layout report.
pub const SCHEMA_VERSION: u32 = 1;

/// Stable identifiers for the implemented layout rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RuleId {
	IconClipped,
	IconOutsideViewport,
	IconsOverlap,
	HiddenPositionedIcon,
	AutomaticArrangement,
	TightGridSpacing,
}

impl RuleId {
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::IconClipped => "icon_clipped",
			Self::IconOutsideViewport => "icon_outside_viewport",
			Self::IconsOverlap => "icons_overlap",
			Self::HiddenPositionedIcon => "hidden_positioned_icon",
			Self::AutomaticArrangement => "automatic_arrangement",
			Self::TightGridSpacing => "tight_grid_spacing",
		}
	}

	#[must_use]
	pub const fn severity(self) -> Severity {
		match self {
			Self::HiddenPositionedIcon => Severity::Info,
			_ => Severity::Warning,
		}
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Severity {
	Info,
	Warning,
}

/// The exact inspected geometry in logical folder points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bounds {
	pub min: [f64; 2],
	pub max: [f64; 2],
}

impl From<FolderRect> for Bounds {
	fn from(rect: FolderRect) -> Self {
		Self {
			min: [rect.min().x(), rect.min().y()],
			max: [rect.max().x(), rect.max().y()],
		}
	}
}

/// A configured icon's virtual destination and placed rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Icon<'input> {
	pub destination: &'input str,
	pub bounds: Bounds,
}

/// Rule-specific evidence derived from the selected preview geometry.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence<'input> {
	IconClipped {
		icon: Icon<'input>,
	},
	IconOutsideViewport {
		icon: Icon<'input>,
	},
	IconsOverlap {
		first: Icon<'input>,
		second: Icon<'input>,
	},
	HiddenPositionedIcon {
		destination: &'input str,
	},
	AutomaticArrangement {
		configured_visible_icons: usize,
	},
	TightGridSpacing {
		spacing_points: f64,
		recommended_minimum_points: f64,
	},
}

impl Evidence<'_> {
	#[must_use]
	pub const fn rule(self) -> RuleId {
		match self {
			Self::IconClipped { .. } => RuleId::IconClipped,
			Self::IconOutsideViewport { .. } => RuleId::IconOutsideViewport,
			Self::IconsOverlap { .. } => RuleId::IconsOverlap,
			Self::HiddenPositionedIcon { .. } => RuleId::HiddenPositionedIcon,
			Self::AutomaticArrangement { .. } => RuleId::AutomaticArrangement,
			Self::TightGridSpacing { .. } => RuleId::TightGridSpacing,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Diagnostic<'input> {
	pub rule: RuleId,
	pub severity: Severity,
	pub evidence: Evidence<'input>,
	/// Present when the caller suppressed this rule. The finding remains visible.
	pub suppression_reason: Option<&'input str>,
}

/// Findings for the configured icon layout of one folder.
#[derive(Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Report<'input> {
	pub schema_version: u32,
	/// Initial content viewport using the configured scroll origin.
	pub viewport: Bounds,
	pub diagnostics: Vec<Diagnostic<'input>>,
}
