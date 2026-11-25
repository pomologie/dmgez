//! Finder metadata capability checks before access to host inputs.

use super::{Availability, BuildFeature, PreflightError, options, preflight, project};

#[test]
fn qualifies_item_icon_capability_before_source_access() {
	let project = project("\n[[contents]]\nsource = 'absent'\ncustom_icon = 'also-absent.icns'\n");
	let mut options = options();
	options.capabilities.finder_custom_icon = Availability::Unavailable;
	let rejected = preflight(&project, options);
	if cfg!(feature = "volume-icon") {
		assert!(matches!(
			rejected,
			Err(PreflightError::BackendUnavailable(
				BuildFeature::FinderCustomIcon
			))
		));
		options.capabilities.finder_custom_icon = Availability::Available;
		preflight(&project, options).unwrap();
	} else {
		assert!(matches!(
			rejected,
			Err(PreflightError::FeatureNotCompiled(
				BuildFeature::FinderCustomIcon
			))
		));
	}
}

#[test]
fn visibility_selections_require_a_finder_flag_backend_before_discovery() {
	let project =
		project("\n[[contents]]\nsource = 'absent'\n[finder]\nhide = ['/Toolkit/Hidden.txt']\n");
	let mut options = options();
	options.capabilities.finder_flags = Availability::Unavailable;
	assert!(matches!(
		preflight(&project, options),
		Err(PreflightError::BackendUnavailable(
			BuildFeature::FinderFlags
		))
	));
}

#[cfg(feature = "finder-write")]
#[test]
fn accepts_unicode_finder_folder_paths_only_in_the_unicode_profile() {
	let project = project("[[finder.folders]]\npath = '/Guidé 🚀'\n");
	let mut options = options();
	options.capabilities.finder_layout = Availability::Available;
	assert!(matches!(
		preflight(&project, options),
		Err(PreflightError::FinderFolderName { index: 0 })
	));
	options.capabilities.hfsplus_unicode = Availability::Available;
	preflight(&project, options).unwrap();
}
