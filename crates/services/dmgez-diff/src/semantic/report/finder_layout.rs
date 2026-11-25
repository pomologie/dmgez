use dmgez_finder::{STORE_NAME, inspection::ReadLimits};
use serde::Serialize;

use super::{
	COMPARED_FIELDS, COMPARED_FIELDS_WITH_XATTRS, ChangeKind, ContentComparison, Field, Policy,
	UNCOMPARED_FIELDS, UNCOMPARED_FIELDS_WITH_XATTRS, Uncompared, replace_finder_exclusion,
	with_field,
};

pub(super) const COMPARED_WITH_LAYOUT: [Field; COMPARED_FIELDS.len() + 1] =
	with_field(&COMPARED_FIELDS, Field::FinderLayout);
pub(super) const COMPARED_WITH_LAYOUT_AND_XATTRS: [Field; COMPARED_FIELDS_WITH_XATTRS.len() + 1] =
	with_field(&COMPARED_FIELDS_WITH_XATTRS, Field::FinderLayout);
const EXCLUDED_WITH_LAYOUT: [Uncompared; UNCOMPARED_FIELDS.len()] =
	replace_finder_exclusion(UNCOMPARED_FIELDS, Uncompared::PerObjectFinderInfo);
const EXCLUDED_WITH_LAYOUT_AND_XATTRS: [Uncompared; UNCOMPARED_FIELDS_WITH_XATTRS.len()] =
	replace_finder_exclusion(
		UNCOMPARED_FIELDS_WITH_XATTRS,
		Uncompared::PerObjectFinderInfo,
	);
const EXCLUDED_LAYOUT_VALUES: &[FinderLayoutUncompared] = &[
	FinderLayoutUncompared::OpaqueRecordValues,
	FinderLayoutUncompared::UnreportedSettingValues,
	FinderLayoutUncompared::UnreportedAliasFields,
	FinderLayoutUncompared::AllocatorLayout,
	FinderLayoutUncompared::AliasResolution,
];

/// Scope and per-store limits for comparing admitted Finder stores.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct FinderLayoutPolicy {
	/// Exact name of matched regular files whose data forks are decoded.
	pub store_name: &'static str,
	pub max_store_bytes: usize,
	pub max_records: usize,
	pub max_name_units: usize,
	pub max_plist_objects: usize,
	pub max_plist_entries: usize,
	pub excluded: &'static [FinderLayoutUncompared],
}

/// Finder store information outside the admitted record comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum FinderLayoutUncompared {
	OpaqueRecordValues,
	UnreportedSettingValues,
	UnreportedAliasFields,
	AllocatorLayout,
	AliasResolution,
}

/// Record name, field FourCC, and type of difference within a matched store.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct FinderRecordChange {
	pub name: String,
	pub field: [u8; 4],
	pub kind: ChangeKind,
}

impl Policy {
	pub(super) fn with_finder_layout(mut self) -> Self {
		let limits = ReadLimits::default();
		self.contents = ContentComparison::ExactBytesWithFinderLayout;
		self.finder_layout = Some(FinderLayoutPolicy {
			store_name: STORE_NAME,
			max_store_bytes: limits.store_bytes,
			max_records: limits.records,
			max_name_units: limits.name_units,
			max_plist_objects: limits.plist_objects,
			max_plist_entries: limits.plist_entries,
			excluded: EXCLUDED_LAYOUT_VALUES,
		});
		let (compared, excluded): (&[Field], &[Uncompared]) = if self.xattrs.is_some() {
			(
				&COMPARED_WITH_LAYOUT_AND_XATTRS,
				&EXCLUDED_WITH_LAYOUT_AND_XATTRS,
			)
		} else {
			(&COMPARED_WITH_LAYOUT, &EXCLUDED_WITH_LAYOUT)
		};
		self.compared = compared;
		self.excluded = excluded;
		self
	}
}
