//! Focused metadata admission evidence for D-BUILD-03 and D-V05.

use std::fs;

use dmgez_config::Project;

use super::*;
use crate::manifest::{Limits, discover};

const UID: u32 = 501;
const GID: u32 = 20;
const MODE: u16 = 0o751;
const FINDER: [u8; FINDER_INFO_BYTES] = [0x5a; FINDER_INFO_BYTES];
const FORK: &[u8] = b"\x00Resource\xff";
const ORDINARY_NAME: &[u8] = b"user.exact";
const UNSUPPORTED_NAME: &[u8] = b"security.opaque\xff";
const VALUE: &[u8] = b"\x00Value\xff";
const FIXTURE_TIMESTAMP: Timestamp = Timestamp {
	seconds: -1,
	nanoseconds: 0,
};
const FLAG_PRESERVED: u32 = 0x04;
const FLAG_UNSUPPORTED: u32 = 0x40;
const REPORT_BUDGET: usize = 4096;
const IDENTITY: SourceIdentity = SourceIdentity {
	device: 2,
	inode: 17,
};

fn source() -> SourceMetadata {
	SourceMetadata {
		identity: IDENTITY,
		size: 1,
		mode: MODE,
		uid: UID,
		gid: GID,
		link_count: 1,
		accessed: FIXTURE_TIMESTAMP,
		modified: FIXTURE_TIMESTAMP,
		changed: FIXTURE_TIMESTAMP,
		created: Some(FIXTURE_TIMESTAMP),
		flags: Some(FLAG_PRESERVED),
	}
}

fn capabilities() -> Capabilities {
	Capabilities {
		permissions: true,
		ownership: true,
		timestamps: Some(TimestampEncoding {
			unix_seconds: -1..=1,
			precision: TimestampPrecision::Seconds,
		}),
		supported_bsd_flags: FLAG_PRESERVED,
		finder_info: true,
		max_resource_fork_bytes: Some(FORK.len() as u64),
		extended_attributes: Some(|name, value_bytes| {
			name == ORDINARY_NAME && value_bytes <= VALUE.len()
		}),
		hard_links: true,
	}
}

fn attributes() -> Attributes {
	Attributes {
		finder_info: Some(FINDER),
		resource_fork: Some(FORK.to_vec()),
		extended: vec![Attribute {
			name: ORDINARY_NAME.to_vec(),
			value: VALUE.to_vec(),
		}],
		accounted_bytes: 1024,
	}
}

#[test]
fn preserves_exact_metadata_with_no_report_allocation() {
	let mut report_bytes = 0;
	let admission = Policy::new(&Metadata::default(), true)
		.admit(
			&source(),
			AttributeCapture::Known(attributes()),
			Some(IDENTITY),
			&capabilities(),
			&mut report_bytes,
		)
		.unwrap();
	assert!(admission.losses.is_empty());
	assert_eq!(admission.metadata.permissions, Some(MODE));
	assert_eq!(admission.metadata.ownership, Some((UID, GID)));
	assert_eq!(admission.metadata.modified, Some(FIXTURE_TIMESTAMP));
	assert_eq!(admission.metadata.bsd_flags, Some(FLAG_PRESERVED));
	assert_eq!(admission.metadata.finder_info, Some(FINDER));
	assert_eq!(admission.metadata.resource_fork.as_deref(), Some(FORK));
	assert_eq!(
		admission.metadata.extended_attributes,
		attributes().extended
	);
	assert_eq!(admission.metadata.hard_link, Some(IDENTITY));
}

#[test]
fn strict_rejects_unobserved_and_unsupported_required_fields() {
	let requests = Metadata::default();
	let policy = Policy::new(&requests, true);
	let mut report_bytes = REPORT_BUDGET;
	let unavailable = policy
		.admit(
			&source(),
			AttributeCapture::Unavailable,
			None,
			&capabilities(),
			&mut report_bytes,
		)
		.unwrap_err();
	assert!(matches!(
		unavailable,
		Error::Loss(Loss {
			field: Field::FinderInfo,
			reason: LossReason::SourceUnavailable,
		})
	));

	let mut known = attributes();
	known.extended.push(Attribute {
		name: UNSUPPORTED_NAME.to_vec(),
		value: VALUE.to_vec(),
	});
	let unsupported = policy
		.admit(
			&source(),
			AttributeCapture::Known(known),
			None,
			&capabilities(),
			&mut report_bytes,
		)
		.unwrap_err();
	assert!(matches!(unsupported, Error::Loss(Loss {
		field: Field::ExtendedAttribute { name, bytes }, reason: LossReason::TargetUnsupported,
	}) if name == UNSUPPORTED_NAME && bytes == VALUE.len()));
	assert_eq!(report_bytes, REPORT_BUDGET);
}

#[test]
fn report_keeps_supported_fields_and_names_each_loss() {
	let mut observed = source();
	observed.modified.nanoseconds = 1;
	observed.changed.seconds = 2;
	observed.flags = Some(FLAG_PRESERVED | FLAG_UNSUPPORTED);
	let mut caps = capabilities();
	caps.hard_links = false;
	caps.max_resource_fork_bytes = Some(1);
	let mut attrs = attributes();
	attrs.extended.push(Attribute {
		name: UNSUPPORTED_NAME.to_vec(),
		value: VALUE.to_vec(),
	});
	let mut report_bytes = REPORT_BUDGET;
	let admission = Policy::new(&Metadata::default(), false)
		.admit(
			&observed,
			AttributeCapture::Known(attrs),
			Some(IDENTITY),
			&caps,
			&mut report_bytes,
		)
		.unwrap();
	assert_eq!(admission.metadata.modified, Some(FIXTURE_TIMESTAMP));
	assert!(admission.metadata.changed.is_none());
	assert_eq!(admission.metadata.bsd_flags, Some(FLAG_PRESERVED));
	assert!(admission.metadata.resource_fork.is_none());
	assert_eq!(admission.metadata.finder_info, Some(FINDER));
	assert!(admission.metadata.hard_link.is_none());
	assert_eq!(
		admission.metadata.extended_attributes,
		attributes().extended
	);
	assert_eq!(
		admission.losses,
		[
			Loss {
				field: Field::Timestamp(TimestampField::Modified),
				reason: LossReason::TimestampPrecision {
					discarded_nanoseconds: 1
				}
			},
			Loss {
				field: Field::Timestamp(TimestampField::Changed),
				reason: LossReason::TimestampOutOfRange
			},
			Loss {
				field: Field::BsdFlags,
				reason: LossReason::UnsupportedBsdFlags {
					bits: FLAG_UNSUPPORTED
				}
			},
			Loss {
				field: Field::HardLink { identity: IDENTITY },
				reason: LossReason::TargetUnsupported
			},
			Loss {
				field: Field::ResourceFork {
					bytes: Some(FORK.len() as u64)
				},
				reason: LossReason::ResourceForkTooLarge { max_bytes: 1 }
			},
			Loss {
				field: Field::ExtendedAttribute {
					name: UNSUPPORTED_NAME.to_vec(),
					bytes: VALUE.len()
				},
				reason: LossReason::TargetUnsupported
			},
		]
	);
	assert!(
		REPORT_BUDGET - report_bytes
			>= size_of::<Loss>() * admission.losses.len() + UNSUPPORTED_NAME.len()
	);
}

#[test]
fn explicit_omission_is_visible_and_does_not_override_strict_failures() {
	let mut requests = Metadata::default();
	requests.preserve_finder_info = false;
	let mut caps = capabilities();
	caps.finder_info = false;
	let mut report_bytes = size_of::<Loss>();
	let policy = Policy::new(&requests, true);
	let admission = policy
		.admit(
			&source(),
			AttributeCapture::Known(attributes()),
			None,
			&caps,
			&mut report_bytes,
		)
		.unwrap();
	assert!(admission.metadata.finder_info.is_none());
	assert_eq!(
		admission.losses,
		[Loss {
			field: Field::FinderInfo,
			reason: LossReason::ConfiguredOmission
		}]
	);
	assert_eq!(report_bytes, 0);
	assert!(matches!(
		policy.admit(
			&source(),
			AttributeCapture::Known(attributes()),
			None,
			&caps,
			&mut report_bytes
		),
		Err(Error::Budget)
	));
}

#[test]
fn strict_distinguishes_optional_stat_absence_from_precision_loss() {
	let requests = Metadata::default();
	let policy = Policy::new(&requests, true);
	let mut observed = source();
	observed.created = None;
	observed.flags = None;
	let mut report_bytes = REPORT_BUDGET;
	let admission = policy
		.admit(
			&observed,
			AttributeCapture::NotApplicable,
			None,
			&capabilities(),
			&mut report_bytes,
		)
		.unwrap();
	assert!(admission.metadata.created.is_none());
	assert!(admission.metadata.bsd_flags.is_none());
	assert_eq!(
		admission.losses,
		[
			Loss {
				field: Field::Timestamp(TimestampField::Created),
				reason: LossReason::OptionalStatUnavailable
			},
			Loss {
				field: Field::BsdFlags,
				reason: LossReason::OptionalStatUnavailable
			},
		]
	);
	observed.modified.nanoseconds = 1;
	assert!(matches!(
		policy.admit(
			&observed,
			AttributeCapture::NotApplicable,
			None,
			&capabilities(),
			&mut report_bytes
		),
		Err(Error::Loss(Loss {
			field: Field::Timestamp(TimestampField::Modified),
			reason: LossReason::TimestampPrecision {
				discarded_nanoseconds: 1
			},
		}))
	));
}

#[test]
fn groups_real_links_without_coalescing_equal_bytes() {
	const PROJECT: &str = "schema_version=1\n[image]\nname='Links'\noutput='image.dmg'\n\
		[[contents]]\nsource='files'\n";
	const CONTENT: &[u8] = b"same bytes";
	let temp = tempfile::tempdir().unwrap();
	let directory = temp.path().canonicalize().unwrap();
	let files = directory.join("files");
	fs::create_dir(&files).unwrap();
	fs::write(files.join("first"), CONTENT).unwrap();
	fs::hard_link(files.join("first"), files.join("second")).unwrap();
	fs::write(files.join("distinct"), CONTENT).unwrap();
	fs::write(files.join("external"), CONTENT).unwrap();
	fs::hard_link(files.join("external"), directory.join("outside")).unwrap();
	let manifest = discover(
		&Project::parse(PROJECT).unwrap(),
		&directory,
		Limits::default(),
	)
	.unwrap();
	let find = |name| {
		manifest
			.entries()
			.iter()
			.find(|entry| entry.destination.as_str() == name)
			.unwrap()
	};
	let bytes = size_of::<SourceIdentity>() * 3;
	assert!(matches!(
		HardLinks::discover(&manifest, bytes - 1),
		Err(Error::Budget)
	));
	let links = HardLinks::discover(&manifest, bytes).unwrap();
	assert_eq!(links.accounted_bytes(), bytes);
	let identity = links.group_for(find("/files/first")).unwrap();
	assert_eq!(links.group_for(find("/files/second")), Some(identity));
	for name in ["/files", "/files/distinct", "/files/external"] {
		assert_eq!(links.group_for(find(name)), None);
	}
}
