use super::*;

#[test]
fn partition_admission_is_feature_gated_and_uses_the_outer_disk_geometry() {
	let planned = project("partition = 'gpt'\nsize = '4MiB'\n");
	let admission = preflight(&planned, options());
	if !cfg!(feature = "partition-write") {
		assert!(matches!(
			admission,
			Err(PreflightError::FeatureNotCompiled(
				BuildFeature::PartitionWrite
			))
		));
		return;
	}
	let profile = admission.unwrap();
	assert_eq!(profile.partition(), Layout::Gpt);
	assert_eq!(profile.partition().overhead_bytes(), 40 << 10);
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
	] {
		assert!(matches!(
			preflight(&project(settings), options()),
			Err(PreflightError::PartitionGeometry)
		));
	}
	let mut unsupported = planned;
	for format in [ImageFormat::Udrw, ImageFormat::Ulfo, ImageFormat::Ulmo] {
		unsupported.image.format = format;
		assert!(matches!(
			preflight(&unsupported, options()),
			Err(PreflightError::PartitionFormat { partition: Partition::Gpt, format: actual })
				if actual == format
		));
	}
}
