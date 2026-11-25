use std::str::FromStr;

use dmgez_preview::FolderRect;
use serde::{Serialize, Serializer};
use thiserror::Error;

/// Version of the serialized layout report.
pub const SCHEMA_VERSION: u32 = 1;

/// Stable identifiers for the implemented layout rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RuleId {
	IconClipped,
	IconOutsideViewport,
	IconsOverlap,
	HiddenPositionedIcon,
	AutomaticArrangement,
	TightGridSpacing,
}

impl RuleId {
	const ALL: [Self; 6] = [
		Self::IconClipped,
		Self::IconOutsideViewport,
		Self::IconsOverlap,
		Self::HiddenPositionedIcon,
		Self::AutomaticArrangement,
		Self::TightGridSpacing,
	];

	/// Returns the stable, case-sensitive rule identifier.
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

/// The supplied spelling is not a supported lint rule identifier.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("unknown layout lint rule identifier")]
pub struct ParseRuleIdError;

impl FromStr for RuleId {
	type Err = ParseRuleIdError;

	fn from_str(value: &str) -> Result<Self, Self::Err> {
		Self::ALL
			.into_iter()
			.find(|rule| rule.as_str() == value)
			.ok_or(ParseRuleIdError)
	}
}

impl Serialize for RuleId {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_unit_variant(stringify!(RuleId), *self as u32, self.as_str())
	}
}

/// Severity of a layout finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Severity {
	Info,
	Warning,
}

impl Severity {
	/// Returns the stable severity name used in reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Info => "info",
			Self::Warning => "warning",
		}
	}
}

impl Serialize for Severity {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_unit_variant(stringify!(Severity), *self as u32, self.as_str())
	}
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
