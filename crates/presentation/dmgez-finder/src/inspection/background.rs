use dmg_layout::{AliasKind, AliasRecordKind, DiskImageError, ExtraEntry, ExtraPayload, ExtrasTag};

const APPLICATION_SIGNATURE_BYTES: usize = size_of::<[u8; 4]>();
const RECORD_LENGTH_END: usize = APPLICATION_SIGNATURE_BYTES + size_of::<u16>();
const VERSION_3: u16 = 3;

/// The stored file reference in an image-background property.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BackgroundAlias {
	/// Catalog and volume identity supplied by the store.
	File(Box<BackgroundAliasTarget>),
	/// The stored background type selects an image, but the alias key is absent.
	Missing,
	/// The property type, alias record, or reported identity fields are invalid.
	Malformed,
	/// The record identifies an unsupported classic alias version.
	UnsupportedVersion(u16),
	/// The record refers to a folder.
	UnsupportedTarget,
}

/// Identity fields recorded by the alias. Treat names and paths as untrusted.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BackgroundAliasTarget {
	pub version: u16,
	pub volume_name: Option<BackgroundAliasName>,
	pub target_name: Option<BackgroundAliasName>,
	/// Raw bytes from the POSIX target-path extra.
	pub target_path: Option<Vec<u8>>,
	pub parent_catalog_id: u32,
	pub target_catalog_id: u32,
	pub volume_created: HfsCreationTime,
	pub target_created: HfsCreationTime,
}

/// Name bytes in their stored representation, preferring the Unicode extra.
#[derive(Debug, Eq, PartialEq)]
pub enum BackgroundAliasName {
	Unicode(String),
	Legacy(Vec<u8>),
}

/// Raw creation time from the alias header, relative to 1904-01-01 UTC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HfsCreationTime {
	/// The version-2 header records whole seconds.
	Seconds(u32),
	/// The version-3 header records units of 1/65536 second.
	FixedPoint(u64),
}

pub(super) fn inspect(bytes: &[u8]) -> BackgroundAlias {
	// Input comes from a property in the admitted single-leaf store. The
	// alias decoder checks its wire length and reads only this property.
	let record = match AliasRecordKind::from_bytes(bytes) {
		Ok(record) => record,
		Err(DiskImageError::UnsupportedVersion { version })
			if recorded_length(bytes) == Some(bytes.len()) =>
		{
			return BackgroundAlias::UnsupportedVersion(version);
		}
		Err(_) => return BackgroundAlias::Malformed,
	};
	match file_target(record) {
		Ok(target) => BackgroundAlias::File(Box::new(target)),
		Err(status) => status,
	}
}

fn recorded_length(bytes: &[u8]) -> Option<usize> {
	let field = bytes.get(APPLICATION_SIGNATURE_BYTES..RECORD_LENGTH_END)?;
	Some(usize::from(u16::from_be_bytes(field.try_into().ok()?)))
}

fn file_target(record: AliasRecordKind) -> Result<BackgroundAliasTarget, BackgroundAlias> {
	match record {
		AliasRecordKind::Version2(record) => {
			let names = ReportedExtras::read(record.header.kind, record.extras())?;
			let header = record.header;
			Ok(BackgroundAliasTarget {
				version: header.version,
				volume_name: names
					.volume_name
					.or(Some(BackgroundAliasName::Legacy(header.volume_name))),
				target_name: names
					.target_name
					.or(Some(BackgroundAliasName::Legacy(header.target_name))),
				target_path: names.target_path,
				parent_catalog_id: header.parent_catalog_node_id.raw(),
				target_catalog_id: header.target_catalog_node_id.raw(),
				volume_created: HfsCreationTime::Seconds(header.volume_date.raw()),
				target_created: HfsCreationTime::Seconds(header.target_date.raw()),
			})
		}
		AliasRecordKind::Version3(record) => {
			let names = ReportedExtras::read(record.header.kind, record.extras())?;
			let header = record.header;
			Ok(BackgroundAliasTarget {
				version: VERSION_3,
				volume_name: names.volume_name,
				target_name: names.target_name,
				target_path: names.target_path,
				parent_catalog_id: header.parent_catalog_node_id.raw(),
				target_catalog_id: header.target_catalog_node_id.raw(),
				volume_created: HfsCreationTime::FixedPoint(header.volume_date.raw()),
				target_created: HfsCreationTime::FixedPoint(header.target_date.raw()),
			})
		}
	}
}

#[derive(Default)]
struct ReportedExtras {
	volume_name: Option<BackgroundAliasName>,
	target_name: Option<BackgroundAliasName>,
	target_path: Option<Vec<u8>>,
}

impl ReportedExtras {
	fn read<'a>(
		kind: AliasKind,
		extras: impl Iterator<Item = &'a ExtraEntry>,
	) -> Result<Self, BackgroundAlias> {
		if kind != AliasKind::File {
			return Err(BackgroundAlias::UnsupportedTarget);
		}
		let mut fields = Self::default();
		for extra in extras {
			// Decode only reported extras. Recursive references, including
			// container aliases, remain opaque.
			if !matches!(
				extra.tag(),
				ExtrasTag::UnicodeVolumeName
					| ExtrasTag::UnicodeTargetName
					| ExtrasTag::PortableOperatingSystemInterfacePath
			) {
				continue;
			}
			let duplicate = match extra.payload().map_err(|_| BackgroundAlias::Malformed)? {
				ExtraPayload::UnicodeVolumeName(name) => fields
					.volume_name
					.replace(BackgroundAliasName::Unicode(name))
					.is_some(),
				ExtraPayload::UnicodeTargetName(name) => fields
					.target_name
					.replace(BackgroundAliasName::Unicode(name))
					.is_some(),
				ExtraPayload::PortableOperatingSystemInterfacePath(path) => {
					fields.target_path.replace(path).is_some()
				}
				_ => return Err(BackgroundAlias::Malformed),
			};
			if duplicate {
				return Err(BackgroundAlias::Malformed);
			}
		}
		Ok(fields)
	}
}
