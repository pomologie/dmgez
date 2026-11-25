use super::*;

#[test]
fn partition_admission_is_feature_gated_and_uses_the_outer_disk_geometry() {
	let planned = project("partition = 'gpt'\nsize = '4MiB'\n");
	let mbr = project("partition = 'mbr'\nsize = '4MiB'\n");
	let admission = preflight(&planned, options());
	if !cfg!(feature = "partition-write") {
		for result in [admission, preflight(&mbr, options())] {
			assert!(matches!(
				result,
				Err(PreflightError::FeatureNotCompiled(
					BuildFeature::PartitionWrite
				))
			));
		}
		return;
	}
	let profile = admission.unwrap();
	assert_eq!(profile.partition(), Layout::Gpt);
	assert_eq!(profile.partition().overhead_bytes(), 40 << 10);
	profile.validate_logical_bytes(4 * MIB).unwrap();
	let profile = preflight(&mbr, options()).unwrap();
	assert_eq!(profile.partition(), Layout::Mbr);
	assert_eq!(profile.partition().overhead_bytes(), 4 << 10);
	profile.validate_logical_bytes(4 * MIB).unwrap();
	assert!(matches!(
		preflight(
			&project("partition = 'gpt'\n[performance]\nmemory_limit = '64KiB'\n"),
			options()
		),
		Err(PreflightError::ResourceLimit(Resource::Memory))
	));
	for settings in [
		"partition = 'gpt'\nsize = '40KiB'\n",
		"partition = 'apm'\nsize = '51712B'\n",
		"partition = 'gpt'\nsize = '32KiB'\nshrink = true\n",
		"partition = 'apm'\nsize = '2TiB'\n",
		"partition = 'mbr'\nsize = '4KiB'\n",
		"partition = 'mbr'\nsize = '4608B'\n",
		"partition = 'mbr'\nsize = '2TiB'\n",
	] {
		assert!(matches!(
			preflight(&project(settings), options()),
			Err(PreflightError::PartitionGeometry)
		));
	}
	let mut raw_backed = planned;
	raw_backed.image.format = ImageFormat::Udrw;
	assert!(matches!(
		preflight(&raw_backed, options()),
		Err(PreflightError::PartitionFormat {
			partition: Partition::Gpt,
			format: ImageFormat::Udrw,
		})
	));
}

#[cfg(feature = "partition-write")]
#[test]
fn partitioned_modern_codecs_require_their_own_features() {
	for (format, codec, profile, compiled) in [
		(
			ImageFormat::Ulfo,
			BuildFeature::Lzfse,
			Profile::Ulfo,
			cfg!(feature = "lzfse"),
		),
		(
			ImageFormat::Ulmo,
			BuildFeature::Lzma,
			Profile::Ulmo,
			cfg!(feature = "lzma"),
		),
	] {
		let mut plan = project("partition = 'gpt'\nsize = '4MiB'\n");
		plan.image.format = format;
		let result = preflight(&plan, options());
		if compiled {
			assert_eq!(result.unwrap().write_options().profile, profile);
		} else {
			assert!(matches!(
				result,
				Err(PreflightError::FeatureNotCompiled(feature)) if feature == codec
			));
		}
	}
}
