//! Pure profile admission evidence for D-BUILD-01-T01.

use super::*;

mod finder_capabilities;
mod partition;
mod raw_output;

const MIB: u64 = 1 << 20;
const HOST_THREADS: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const IMAGE: &str =
	"schema_version = 1\n[image]\nname = 'Example'\noutput = 'example.dmg'\nformat = 'udro'\n";
const CACHE: &str =
	"[performance.cache]\ndirectory = 'deliberately-absent-cache'\nmax_bytes = '1MiB'\n";

fn project(settings: &str) -> Project {
	Project::parse(&format!("{IMAGE}{settings}")).unwrap()
}

fn options() -> PreflightOptions {
	PreflightOptions {
		capabilities: BuildCapabilities {
			hfsplus_ascii: Availability::Available,
			..BuildCapabilities::default()
		},
		output: OutputIntent::Path,
		release: ReleaseIntent::Unsigned,
		host_threads: HOST_THREADS,
	}
}

#[test]
fn admits_limits_and_geometry_without_opening_sources() {
	let project = project(
		"size = '8MiB'\nfree_space = '1MiB'\n\
		[[contents]]\nsource = 'does-not-exist/Example.app'\n\
		[performance]\nparallel = false\nmax_entries = 4\nmax_runs = 8\n\
		[build]\nreproducible = true\nsource_date_epoch = 1700000000\n",
	);
	let profile = preflight(&project, options()).unwrap();
	assert_eq!(profile.filesystem(), FilesystemProfile::HfsplusAscii);
	assert_eq!(profile.free_space_bytes(), MIB);
	assert_eq!(profile.write_options().profile, Profile::Udro);
	assert_eq!(profile.write_options().execution, Execution::Serial);
	assert_eq!(profile.output(), OutputPlan::Path { overwrite: false });
	assert_eq!(profile.limits().max_entries, 4);
	assert_eq!(profile.limits().max_runs, 8);
	assert!(profile.reproducible());
	assert!(profile.fail_on_metadata_loss());
	assert_eq!(profile.source_date_epoch(), Some(1_700_000_000));
	profile.validate_logical_bytes(8 * MIB).unwrap();
	assert!(matches!(
		profile.validate_logical_bytes(7 * MIB),
		Err(PreflightError::Capacity)
	));
}

#[test]
fn rejects_declared_and_final_sizes_outside_admitted_run_budget() {
	assert!(matches!(
		preflight(
			&project("size = '8MiB'\n[performance]\nmax_runs = 7\n"),
			options()
		),
		Err(PreflightError::ResourceLimit(Resource::Runs))
	));
	let shrink = preflight(
		&project("size = '8MiB'\nshrink = true\n[performance]\nmax_runs = 7\n"),
		options(),
	)
	.unwrap();
	shrink.validate_logical_bytes(7 * MIB).unwrap();
	assert!(matches!(
		shrink.validate_logical_bytes(8 * MIB),
		Err(PreflightError::ResourceLimit(Resource::Runs))
	));
	assert!(matches!(
		shrink.validate_logical_bytes(9 * MIB),
		Err(PreflightError::Capacity)
	));
}

#[test]
fn rejects_unsupported_filesystems_formats_partitions_and_catalog_names() {
	for filesystem in [Filesystem::Hfsx, Filesystem::Apfs] {
		let mut project = project("");
		project.image.filesystem = filesystem;
		assert!(matches!(
			preflight(&project, options()),
			Err(PreflightError::Filesystem(actual)) if actual == filesystem
		));
	}
	let mut unsupported = project("");
	unsupported.image.partition = Partition::Mbr;
	assert!(matches!(
		preflight(&unsupported, options()),
		Err(PreflightError::Partition(Partition::Mbr))
	));
	unsupported.image.partition = Partition::None;
	unsupported.image.format = ImageFormat::Raw;
	assert!(matches!(
		preflight(&unsupported, options()),
		Err(PreflightError::Format(ImageFormat::Raw))
	));
	unsupported.image.format = ImageFormat::Udro;
	unsupported.compression.level = Some(5);
	assert!(matches!(
		preflight(&unsupported, options()),
		Err(PreflightError::Compression)
	));
	let unicode = project("[[contents]]\ntype = 'directory'\ndestination = '/Échantillon'\n");
	assert!(matches!(
		preflight(&unicode, options()),
		Err(PreflightError::Destination { index: 0 })
	));
	let mut unicode_options = options();
	unicode_options.capabilities.hfsplus_unicode = Availability::Available;
	assert_eq!(
		preflight(&unicode, unicode_options).unwrap().filesystem(),
		FilesystemProfile::HfsplusUnicode
	);
}

#[test]
fn distinguishes_disabled_and_unavailable_backend() {
	let mut options = options();
	options.capabilities.hfsplus_ascii = Availability::NotCompiled;
	assert!(matches!(
		preflight(&project(""), options),
		Err(PreflightError::FeatureNotCompiled(BuildFeature::Hfsplus))
	));
	options.capabilities.hfsplus_ascii = Availability::Unavailable;
	assert!(matches!(
		preflight(&project(""), options),
		Err(PreflightError::BackendUnavailable(BuildFeature::Hfsplus))
	));
}

#[test]
fn staged_verification_and_release_require_explicit_capability_and_path() {
	let mut options = options();
	let verify = project("[build]\nverify_output = true\n");
	assert!(matches!(
		preflight(&verify, options),
		Err(PreflightError::FeatureNotCompiled(BuildFeature::Verify))
	));
	options.capabilities.verify = Availability::Available;
	assert!(preflight(&verify, options).unwrap().verify_output());
	let overwrite = project("[build]\noverwrite = true\n");
	assert!(matches!(
		preflight(&overwrite, options),
		Err(PreflightError::FeatureNotCompiled(
			BuildFeature::AtomicReplace
		))
	));
	options.capabilities.atomic_replace = Availability::Available;
	assert_eq!(
		preflight(&overwrite, options).unwrap().output(),
		OutputPlan::Path { overwrite: true }
	);
	options.output = OutputIntent::Stream;
	assert!(matches!(
		preflight(&verify, options),
		Err(PreflightError::VerifyStream)
	));
	assert!(matches!(
		preflight(&overwrite, options),
		Err(PreflightError::OverwriteStream)
	));
	let signing = project("[signing]\nenabled = true\n");
	assert_eq!(
		preflight(&signing, options).unwrap().release(),
		ReleasePlan::ConfiguredStagesDeferred
	);
	options.release = ReleaseIntent::Release;
	assert!(matches!(
		preflight(&signing, options),
		Err(PreflightError::ReleaseStream)
	));
	options.output = OutputIntent::Path;
	assert!(matches!(
		preflight(&signing, options),
		Err(PreflightError::FeatureNotCompiled(BuildFeature::Release))
	));
	options.capabilities.release = Availability::Available;
	assert_eq!(
		preflight(&signing, options).unwrap().release(),
		ReleasePlan::Requested
	);
}

#[test]
fn scheduling_checks_explicit_requests_and_admits_budget_bounded_auto_width() {
	assert!(matches!(
		preflight(
			&project("[performance]\nparallel = false\nthreads = 2\n"),
			options()
		),
		Err(PreflightError::SerialWorkers)
	));
	let requested = project("[performance]\nparallel = true\nthreads = 2\n");
	let auto = project("[performance]\nmemory_limit = '4MiB'\n");
	let auto_profile = preflight(&auto, options()).unwrap();
	let execution = auto_profile.write_options().execution;
	if cfg!(feature = "parallel") {
		let explicit = preflight(&requested, options()).unwrap();
		assert_eq!(
			explicit.write_options().execution,
			Execution::Parallel { workers: 2 }
		);
		assert_eq!(execution, Execution::Parallel { workers: 3 });
	} else {
		assert!(matches!(
			preflight(&requested, options()),
			Err(PreflightError::FeatureNotCompiled(BuildFeature::Parallel))
		));
		assert_eq!(execution, Execution::Serial);
	}
	assert!(matches!(
		preflight(
			&project("[performance]\nmemory_limit = '1MiB'\n"),
			options()
		),
		Err(PreflightError::Writer(dmgez_udif::Error::ResourceLimit(_)))
	));
}

#[test]
fn zlib_availability_is_checked_before_source_resolution() {
	let mut project = project("");
	project.image.format = ImageFormat::Udzo;
	if cfg!(feature = "zlib") {
		assert_eq!(
			preflight(&project, options())
				.unwrap()
				.write_options()
				.profile,
			Profile::Udzo
		);
	} else {
		assert!(matches!(
			preflight(&project, options()),
			Err(PreflightError::FeatureNotCompiled(BuildFeature::Zlib))
		));
	}
}

#[test]
fn compression_cache_requires_an_unsigned_udzo_build_with_a_qualified_backend() {
	let mut project = project(CACHE);
	assert!(matches!(
		preflight(&project, options()),
		Err(PreflightError::CacheFormat)
	));

	project.image.format = ImageFormat::Udzo;
	let result = preflight(&project, options());
	if !cfg!(feature = "zlib") {
		assert!(matches!(
			result,
			Err(PreflightError::FeatureNotCompiled(BuildFeature::Zlib))
		));
	} else if !cfg!(feature = "compression-cache") {
		assert!(matches!(
			result,
			Err(PreflightError::FeatureNotCompiled(
				BuildFeature::CompressionCache
			))
		));
	} else if !cfg!(any(target_os = "linux", target_os = "macos")) {
		assert!(matches!(
			result,
			Err(PreflightError::BackendUnavailable(
				BuildFeature::CompressionCache
			))
		));
	} else {
		assert_eq!(result.unwrap().cache(), project.performance.cache.as_ref());
	}
}

#[test]
fn release_admission_bypasses_the_configured_cache() {
	let mut release = options();
	release.capabilities.release = Availability::Available;
	release.release = ReleaseIntent::Release;
	let profile = preflight(&project(CACHE), release).unwrap();
	assert_eq!(profile.release(), ReleasePlan::Requested);
	assert!(profile.cache().is_none());
}

#[test]
fn ulmo_availability_is_checked_before_source_resolution() {
	let mut project = project("");
	project.image.format = ImageFormat::Ulmo;
	if cfg!(feature = "lzma") {
		assert_eq!(
			preflight(&project, options())
				.unwrap()
				.write_options()
				.profile,
			Profile::Ulmo
		);
	} else {
		assert!(matches!(
			preflight(&project, options()),
			Err(PreflightError::FeatureNotCompiled(BuildFeature::Lzma))
		));
	}
}

#[test]
fn finder_requirements_are_never_silently_dropped() {
	let project = project("[finder]\nwindow_size = [480, 280]\n");
	assert!(matches!(
		preflight(&project, options()),
		Err(PreflightError::FeatureNotCompiled(
			BuildFeature::FinderLayout
		))
	));
}

#[cfg(feature = "finder-write")]
#[test]
fn finder_validates_explicit_options_and_duplicate_positions_before_resolution() {
	let mut options = options();
	options.capabilities.finder_layout = Availability::Available;
	let background = project("[finder]\nbackground = { kind = 'color', value = '#CFB' }\n");
	assert_eq!(
		preflight(&background, options).unwrap().finder(),
		FinderPlan::IconView
	);
	let coverflow = project("[finder]\ndefault_view = 'coverflow'\n");
	assert!(matches!(
		preflight(&coverflow, options),
		Err(PreflightError::Finder(dmgez_finder::Error::View(
			dmgez_config::View::Coverflow
		)))
	));
	let duplicate = project(
		"[[contents]]\nsource = 'Example.app'\nposition = [40, 32]\n\
		[finder.icon_view.positions]\n'example.APP' = [65, 80]\n",
	);
	assert!(matches!(
		preflight(&duplicate, options),
		Err(PreflightError::DuplicatePosition { index: 0 })
	));
	let duplicate_folder = project(
		"[[finder.folders]]\npath = '/Applications'\n\
		[[finder.folders]]\npath = '/applications'\n",
	);
	assert!(matches!(
		preflight(&duplicate_folder, options),
		Err(PreflightError::DuplicateFinderFolder { index: 1 })
	));
}

#[cfg(feature = "finder-write")]
#[test]
fn finder_admits_column_view_without_icon_or_list_settings() {
	let mut options = options();
	options.capabilities.finder_layout = Availability::Available;
	let column_view = project(
		"[finder]\ndefault_view = 'column-view'\n\
		include_icon_view_settings = false\ninclude_list_view_settings = false\n",
	);
	assert_eq!(
		preflight(&column_view, options).unwrap().finder(),
		FinderPlan::ColumnView
	);
}

#[cfg(feature = "finder-write")]
#[test]
fn finder_admits_list_folders_against_their_inherited_view() {
	let mut options = options();
	options.capabilities.finder_layout = Availability::Available;
	let admitted = project(
		"[finder]\ndefault_view = 'list-view'\n\
		[[finder.folders]]\npath = '/Documentation'\ninclude_icon_view_settings = false\n",
	);
	assert_eq!(
		preflight(&admitted, options).unwrap().finder(),
		FinderPlan::ListView
	);

	let contradicted = project(
		"[finder]\ndefault_view = 'list-view'\n\
		[[finder.folders]]\npath = '/Documentation'\ninclude_list_view_settings = false\n",
	);
	assert!(matches!(
		preflight(&contradicted, options),
		Err(PreflightError::Finder(
			dmgez_finder::Error::ListViewExcluded
		))
	));
}

#[test]
fn preflights_opt_in_volume_icon_representations_without_reading_sources() {
	let mut options = options();
	options.capabilities.finder_volume_icon = Availability::Available;
	let source = project("[volume_icon]\nsource = 'absent.icns'\n");
	let images = project("[volume_icon.images]\n'16@2x' = 'absent.png'\n");
	if cfg!(feature = "volume-icon") {
		let admitted = preflight(&source, options).unwrap();
		assert_eq!(
			admitted.volume_icon_source().unwrap().as_path(),
			std::path::Path::new("absent.icns")
		);
		assert_eq!(admitted.finder(), FinderPlan::NoStore);
		assert!(admitted.volume_icon_images().is_none());
		let admitted = preflight(&images, options).unwrap();
		assert_eq!(
			admitted.volume_icon_images().unwrap()[&dmgez_config::IconSlot::SmallRetina].as_path(),
			std::path::Path::new("absent.png")
		);
		assert!(admitted.volume_icon_source().is_none());
		assert_eq!(admitted.finder(), FinderPlan::NoStore);
		options.capabilities.finder_volume_icon = Availability::Unavailable;
		for requested in [&source, &images] {
			assert!(matches!(
				preflight(requested, options),
				Err(PreflightError::BackendUnavailable(
					BuildFeature::FinderVolumeIcon
				))
			));
		}
	} else {
		for requested in [&source, &images] {
			assert!(matches!(
				preflight(requested, options),
				Err(PreflightError::FeatureNotCompiled(
					BuildFeature::FinderVolumeIcon
				))
			));
		}
	}
}

#[test]
fn preflights_the_opt_in_volume_icon_badge_without_reading_sources() {
	let mut options = options();
	options.capabilities.finder_volume_icon = Availability::Available;
	let badge = project("[volume_icon]\nbadge = 'absent.png'\n");
	if !cfg!(feature = "badge-icons") {
		assert!(matches!(
			preflight(&badge, options),
			Err(PreflightError::FeatureNotCompiled(
				BuildFeature::FinderVolumeIconBadge
			))
		));
		return;
	}
	let admitted = preflight(&badge, options).unwrap();
	assert_eq!(
		admitted.volume_icon_badge().unwrap().as_path(),
		std::path::Path::new("absent.png")
	);
	assert!(admitted.volume_icon_source().is_none());
	assert!(admitted.volume_icon_images().is_none());
	options.capabilities.finder_volume_icon = Availability::Unavailable;
	assert!(matches!(
		preflight(&badge, options),
		Err(PreflightError::BackendUnavailable(
			BuildFeature::FinderVolumeIcon
		))
	));
}
