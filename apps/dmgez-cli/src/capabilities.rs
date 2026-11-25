use super::{
	ANALYZE_COMMAND, BUILD_PROJECT_COMMAND, BUILD_RELEASE_CAPABILITY, COMPILED, CONVERT_COMMAND,
	HFSPLUS_DECMPFS_CAPABILITY, IMPORT_APPDMG_COMMAND, INSPECT_COMMAND, INSTALLER_LINT_COMPILED,
	INSTALLER_LINT_FEATURE, LINT_COMMAND, LZFSE_FEATURE, LZMA_FEATURE, NOT_COMPILED,
	OPTIMIZE_COMMAND, PARALLEL_FEATURE, UNWRAP_RAW_COMMAND, VERIFY_APFS_CAPABILITY,
	VERIFY_CONTAINER_CAPABILITY, VERIFY_HFSPLUS_CAPABILITY, WRAP_RAW_COMMAND, diff, extract,
	image_edit, install, installer_test, notarize, project_edit, signing, volume_commands,
};

const COMPRESSION_CACHE_CAPABILITY: &str = "compression-cache";
const HFSPLUS_ANALYSIS_CAPABILITY: &str = "hfsplus-analysis";

pub(super) fn write() {
	let report_field = |name: &str, value: &str| println!("{name} = {value}");
	for (name, compiled) in [
		(BUILD_PROJECT_COMMAND, cfg!(feature = "project-build")),
		(BUILD_RELEASE_CAPABILITY, cfg!(feature = "project-release")),
		(
			COMPRESSION_CACHE_CAPABILITY,
			cfg!(all(
				feature = "compression-cache",
				any(target_os = "linux", target_os = "macos")
			)),
		),
		(project_edit::FEATURE, cfg!(feature = "project-edit")),
		(image_edit::FEATURE, cfg!(feature = "image-edit")),
		(WRAP_RAW_COMMAND, cfg!(feature = "build")),
		(
			PARALLEL_FEATURE,
			cfg!(all(
				any(feature = "build", feature = "convert"),
				feature = "parallel"
			)),
		),
		(
			LZFSE_FEATURE,
			cfg!(all(
				feature = "lzfse",
				any(feature = "build", feature = "inspect", feature = "diff")
			)),
		),
		(
			LZMA_FEATURE,
			cfg!(all(
				feature = "lzma",
				any(feature = "build", feature = "inspect", feature = "diff")
			)),
		),
		(INSPECT_COMMAND, cfg!(feature = "inspect")),
		(VERIFY_HFSPLUS_CAPABILITY, cfg!(feature = "hfsplus-verify")),
		(VERIFY_APFS_CAPABILITY, cfg!(feature = "apfs-read")),
		(
			HFSPLUS_DECMPFS_CAPABILITY,
			cfg!(feature = "hfsplus-decmpfs"),
		),
		(
			volume_commands::HFSPLUS_READ_FEATURE,
			cfg!(feature = "hfsplus-read"),
		),
		(
			volume_commands::APFS_READ_FEATURE,
			cfg!(feature = "apfs-read"),
		),
		(extract::HFSPLUS_FEATURE, extract::HFSPLUS_COMPILED),
		(extract::APFS_FEATURE, extract::APFS_COMPILED),
		(install::HFSPLUS_FEATURE, install::HFSPLUS_COMPILED),
		(install::APFS_FEATURE, install::APFS_COMPILED),
		(installer_test::FEATURE, installer_test::COMPILED),
		(ANALYZE_COMMAND, cfg!(feature = "inspect")),
		(
			HFSPLUS_ANALYSIS_CAPABILITY,
			cfg!(feature = "hfsplus-analysis"),
		),
		(CONVERT_COMMAND, cfg!(feature = "convert")),
		(OPTIMIZE_COMMAND, cfg!(feature = "convert")),
		(diff::COMMAND, cfg!(feature = "diff")),
		(diff::HFSPLUS_CAPABILITY, diff::HFSPLUS_COMPILED),
		(diff::APFS_CAPABILITY, diff::APFS_COMPILED),
		(
			diff::DIFF_HASH_FEATURE,
			cfg!(feature = "diff-hash") && (diff::HFSPLUS_COMPILED || diff::APFS_COMPILED),
		),
		(
			diff::FINDER_DIFF_FEATURE,
			cfg!(feature = "finder-diff") && (diff::HFSPLUS_COMPILED || diff::APFS_COMPILED),
		),
		(IMPORT_APPDMG_COMMAND, cfg!(feature = "appdmg-import")),
		(LINT_COMMAND, cfg!(feature = "lint")),
		(INSTALLER_LINT_FEATURE, INSTALLER_LINT_COMPILED),
		(VERIFY_CONTAINER_CAPABILITY, cfg!(feature = "inspect")),
		(UNWRAP_RAW_COMMAND, cfg!(feature = "inspect")),
	] {
		report_field(name, if compiled { COMPILED } else { NOT_COMPILED });
	}
	report_field(signing::NATIVE_FEATURE, signing::NATIVE_CAPABILITY);
	report_field(notarize::NATIVE_FEATURE, notarize::NATIVE_CAPABILITY);
}
