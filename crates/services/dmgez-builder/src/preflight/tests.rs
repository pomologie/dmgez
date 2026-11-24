//! Pure profile admission evidence for D-BUILD-01-T01.

use super::*;

const MIB: u64 = 1 << 20;
const HOST_THREADS: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const IMAGE: &str =
	"schema_version = 1\n[image]\nname = 'Example'\noutput = 'example.dmg'\nformat = 'udro'\n";

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
	unsupported.image.partition = Partition::Gpt;
	assert!(matches!(
		preflight(&unsupported, options()),
		Err(PreflightError::Partition(Partition::Gpt))
	));
	unsupported.image.partition = Partition::None;
	unsupported.image.format = ImageFormat::Udrw;
	assert!(matches!(
		preflight(&unsupported, options()),
		Err(PreflightError::Format(ImageFormat::Udrw))
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
	options.capabilities.finder_icon_view = Availability::Available;
	let background = project("[finder]\nbackground = { kind = 'color', value = '#CFB' }\n");
	assert_eq!(
		preflight(&background, options).unwrap().finder(),
		FinderPlan::IconView
	);
	let list_view = project("[finder]\ndefault_view = 'list-view'\n");
	assert!(matches!(
		preflight(&list_view, options),
		Err(PreflightError::Finder(dmgez_finder::Error::View(_)))
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
