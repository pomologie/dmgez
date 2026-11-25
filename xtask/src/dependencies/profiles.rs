use super::UDIF_PACKAGE;

const APPLE_CODESIGN: &str = "apple-codesign";
const DMGEZ_MACHO: &str = "dmgez-macho";
const UDIF_READ: &str = "read";
const UDIF_WRITE: &str = "write";
const UDIF_PARALLEL: &str = "parallel";
const UDIF_ZLIB: &str = "zlib";
const UDIF_LZFSE: &str = "lzfse";
const UDIF_LZMA: &str = "lzma";

const PORTABLE_SIGNING_PACKAGE: Option<&str> = if cfg!(any(
	target_os = "macos",
	target_os = "linux",
	target_os = "windows"
)) {
	Some(APPLE_CODESIGN)
} else {
	None
};
const NATIVE_SIGNING_PACKAGE: Option<&str> = if cfg!(target_os = "macos") {
	Some(DMGEZ_MACHO)
} else {
	None
};

const NATIVE_SIGNING_DEPS: &[&str] = &[
	"dmgez-core",
	"dmgez-signing",
	UDIF_PACKAGE,
	"serde_json",
	"sha2",
];
const UDIF_READER: &[&str] = &[UDIF_READ, UDIF_ZLIB];
const UDIF_WRITER: &[&str] = &[UDIF_WRITE, UDIF_ZLIB];
const UDIF_CONVERTER: &[&str] = &[UDIF_READ, UDIF_WRITE, UDIF_ZLIB];
const UDIF_PARALLEL_CONVERTER: &[&str] = &[UDIF_READ, UDIF_WRITE, UDIF_PARALLEL, UDIF_ZLIB];

#[derive(Clone, Copy)]
pub(super) enum Selection {
	Default,
	Minimal,
	Only(&'static str),
}

pub(super) struct Profile {
	pub(super) name: &'static str,
	pub(super) selection: Selection,
	pub(super) packages: &'static [&'static str],
	pub(super) host_package: Option<&'static str>,
	pub(super) udif_features: &'static [&'static str],
}

impl Profile {
	const fn new(
		name: &'static str,
		selection: Selection,
		packages: &'static [&'static str],
		udif_features: &'static [&'static str],
	) -> Self {
		Self {
			name,
			selection,
			packages,
			host_package: None,
			udif_features,
		}
	}

	const fn with_host_package(mut self, package: Option<&'static str>) -> Self {
		self.host_package = package;
		self
	}
}

pub(super) const PROFILES: &[Profile] = &[
	Profile::new("minimal", Selection::Minimal, &[], &[]),
	Profile::new(
		"serial",
		Selection::Only("build"),
		&["dmgez-core", "dmgez-runtime", UDIF_PACKAGE, "flate2"],
		UDIF_WRITER,
	),
	Profile::new(
		"default",
		Selection::Default,
		&[
			"dmgez-core",
			"dmgez-runtime",
			"dmgez-signing",
			UDIF_PACKAGE,
			"flate2",
			"rayon",
			"serde_json",
			"sha2",
		],
		UDIF_PARALLEL_CONVERTER,
	)
	.with_host_package(PORTABLE_SIGNING_PACKAGE),
	Profile::new(
		"inspection",
		Selection::Only("inspect"),
		&[
			"dmgez-core",
			"dmgez-inspector",
			"dmgez-runtime",
			UDIF_PACKAGE,
			"dmgez-vfs",
			"flate2",
			"serde_json",
		],
		UDIF_READER,
	),
	Profile::new(
		"remote_inspection",
		Selection::Only("remote"),
		&[
			"dmgez-core",
			"dmgez-inspector",
			"dmgez-runtime",
			UDIF_PACKAGE,
			"dmgez-vfs",
			"flate2",
			"rustls",
			"serde_json",
			"ureq",
		],
		UDIF_READER,
	),
	Profile::new(
		"conversion",
		Selection::Only("convert"),
		&[
			"dmgez-core",
			"dmgez-runtime",
			"dmgez-operations",
			UDIF_PACKAGE,
			"flate2",
		],
		UDIF_CONVERTER,
	),
	Profile::new(
		"parallel_conversion",
		Selection::Only("convert,parallel"),
		&[
			"dmgez-core",
			"dmgez-runtime",
			"dmgez-operations",
			UDIF_PACKAGE,
			"flate2",
			"rayon",
		],
		UDIF_PARALLEL_CONVERTER,
	),
	Profile::new(
		"comparison",
		Selection::Only("diff"),
		&[
			"dmgez-core",
			"dmgez-runtime",
			"dmgez-diff",
			UDIF_PACKAGE,
			"dmgez-vfs",
			"flate2",
			"serde_json",
		],
		UDIF_READER,
	),
	Profile::new(
		"hash_comparison",
		Selection::Only("diff-hash"),
		&[
			"blake3",
			"dmgez-core",
			"dmgez-runtime",
			"dmgez-diff",
			UDIF_PACKAGE,
			"dmgez-vfs",
			"flate2",
			"serde_json",
		],
		UDIF_READER,
	),
	Profile::new(
		"finder_comparison",
		Selection::Only("finder-diff"),
		&[
			"dmgez-config",
			"dmgez-core",
			"dmgez-runtime",
			"dmgez-diff",
			"dmgez-finder",
			UDIF_PACKAGE,
			"dmgez-vfs",
			"ds_parser",
			"flate2",
			"serde_json",
		],
		UDIF_READER,
	),
	Profile::new(
		"layout_lint",
		Selection::Only("lint"),
		&[
			"dmgez-config",
			"dmgez-runtime",
			"dmgez-lint",
			"dmgez-preview",
			"serde_json",
		],
		&[],
	),
	Profile::new(
		"design_preview",
		Selection::Only("preview"),
		&[
			"dmgez-config",
			"dmgez-runtime",
			"dmgez-core",
			"dmgez-preview",
			"epaint",
			"flate2",
			"png",
		],
		&[],
	),
	Profile::new(
		"desktop_open",
		Selection::Only("desktop-open"),
		&["dmgez-runtime"],
		&[],
	),
	Profile::new(
		"installer_lint",
		Selection::Only("installer-lint"),
		&[
			"dmgez-builder",
			"dmgez-config",
			"dmgez-runtime",
			"dmgez-core",
			"dmgez-lint",
			"dmgez-preview",
			UDIF_PACKAGE,
			"serde_json",
		],
		&[UDIF_WRITE],
	),
	Profile::new(
		"appdmg_import",
		Selection::Only("appdmg-import"),
		&["dmgez-config", "dmgez-runtime", "dmgez-core", "serde_json"],
		&[],
	),
	Profile::new(
		"rust_signing",
		Selection::Only("signing-rust"),
		&[
			"dmgez-core",
			"dmgez-signing",
			UDIF_PACKAGE,
			"flate2",
			"serde_json",
			"sha2",
		],
		UDIF_READER,
	)
	.with_host_package(PORTABLE_SIGNING_PACKAGE),
	Profile::new(
		"native_signing",
		Selection::Only("signing-native"),
		NATIVE_SIGNING_DEPS,
		&[],
	)
	.with_host_package(NATIVE_SIGNING_PACKAGE),
	Profile::new(
		"native_notarization",
		Selection::Only("notarization-native"),
		NATIVE_SIGNING_DEPS,
		&[],
	)
	.with_host_package(NATIVE_SIGNING_PACKAGE),
];

pub(super) const UDIF_FEATURES: &[&str] = &[
	UDIF_READ,
	UDIF_WRITE,
	UDIF_PARALLEL,
	UDIF_ZLIB,
	UDIF_LZFSE,
	UDIF_LZMA,
];

pub(super) const CHECKED_PACKAGES: &[&str] = &[
	APPLE_CODESIGN,
	"blake3",
	"dmgez-builder",
	"dmgez-config",
	"dmgez-core",
	"dmgez-diff",
	"dmgez-finder",
	"dmgez-inspector",
	"dmgez-lint",
	DMGEZ_MACHO,
	"dmgez-operations",
	"dmgez-preview",
	"dmgez-runtime",
	"dmgez-signing",
	UDIF_PACKAGE,
	"dmgez-vfs",
	"ds_parser",
	"epaint",
	"flate2",
	"png",
	"rayon",
	"rustls",
	"serde_json",
	"sha2",
	"ureq",
];

pub(super) const FORBIDDEN_PACKAGES: &[&str] = &[
	"apple-dmg",
	"apfs",
	"apfs-container",
	"apfs-core",
	"apfs-format",
	"apfs-reader",
	"bzip2",
	"dmgez-apfs",
	"dmgez-cache",
	"dmgez-hfsplus",
	"dmgez-mount",
	"eframe",
	"egui",
	"fstool",
	"fuser",
	"hfsplus-reader",
	"lzfse_rust",
	"lzma-rust2",
	"notify",
	"reqwest",
	"rmcp",
	"tokio",
	"udif",
	"winfsp",
];
