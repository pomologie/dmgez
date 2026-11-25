/// Which Finder metadata was available for the selected directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StoreStatus {
	Absent,
	Unavailable,
	Unsupported,
	Malformed,
	Observed,
}

impl StoreStatus {
	/// Stable token for reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Absent => "absent",
			Self::Unavailable => "unavailable",
			Self::Unsupported => "unsupported",
			Self::Malformed => "malformed",
			Self::Observed => "observed",
		}
	}
}

/// How the output obtained its icon layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LayoutStatus {
	/// Manual icon settings and stored centers were available for the selected entries.
	ObservedIcon,
	/// Some settings or icon centers use the preview defaults.
	PartialIcon,
	/// The fallback grid was used because the store could not be observed.
	GenericNoStore,
	/// The fallback grid was used because the store explicitly selects another view.
	GenericOtherView,
	/// The fallback grid was used because the store selects an automatic arrangement.
	GenericAutomaticArrangement,
	/// The fallback grid was used because one or more stored settings were unusable.
	GenericInvalidSettings,
}

impl LayoutStatus {
	/// Stable token for reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::ObservedIcon => "observed_icon",
			Self::PartialIcon => "partial_icon",
			Self::GenericNoStore => "generic_no_store",
			Self::GenericOtherView => "generic_other_view",
			Self::GenericAutomaticArrangement => "generic_automatic_arrangement",
			Self::GenericInvalidSettings => "generic_invalid_settings",
		}
	}
}

/// Whether the observed background could be represented in the output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BackgroundStatus {
	Default,
	ObservedColor,
	ObservedPng,
	UnavailableImage,
	UnsupportedSetting,
	InvalidSetting,
	UnsupportedImage,
	MalformedImage,
}

impl BackgroundStatus {
	/// Stable token for reports.
	#[must_use]
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Default => "default",
			Self::ObservedColor => "observed_color",
			Self::ObservedPng => "observed_png",
			Self::UnavailableImage => "unavailable_image",
			Self::UnsupportedSetting => "unsupported_setting",
			Self::InvalidSetting => "invalid_setting",
			Self::UnsupportedImage => "unsupported_image",
			Self::MalformedImage => "malformed_image",
		}
	}
}

/// Observations and fallbacks for the selected directory and visibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Coverage {
	pub store: StoreStatus,
	pub layout: LayoutStatus,
	pub background: BackgroundStatus,
	/// Number of entries selected by the visibility policy with an exact-name stored center.
	pub observed_positions: usize,
	/// Number of entries selected by the visibility policy assigned a preview grid position.
	pub fallback_positions: usize,
}
