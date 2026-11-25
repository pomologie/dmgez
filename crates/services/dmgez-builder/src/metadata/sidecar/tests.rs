use std::{fs, num::NonZeroUsize, os::unix::fs::symlink};

use dmgez_config::Project;
use dmgez_udif::writer::Cancellation;

use crate::{
	metadata::policy::AttributeCapture,
	planning::{self, PreparedBuild},
	preflight::{Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent},
};

use super::*;

const VERSION_AND_MAGIC: [u8; 8] = [0, 5, 0x16, 7, 0, 2, 0, 0];
const RESOURCE_FORK_ID: u32 = 2;
const ONE_ENTRY_BYTES: u32 = 38;
const RECORD_START_FIELD: usize = 30;
const RESOURCE: &[u8] = b"exact resource-fork data";
const NATIVE_CONFLICT: &[u8] = b"different native fork";
const SOURCE_NAME: &str = "manual.txt";
const SIDECAR_NAME: &str = "._manual.txt";
const OTHER_NAME: &str = "._other.txt";
const TRANSFER_BUDGET: usize = 4096;

fn classic(resource: &[u8]) -> Vec<u8> {
	let mut bytes = VERSION_AND_MAGIC.to_vec();
	bytes.extend_from_slice(&[0; 16]);
	bytes.extend_from_slice(&1_u16.to_be_bytes());
	bytes.extend_from_slice(&RESOURCE_FORK_ID.to_be_bytes());
	bytes.extend_from_slice(&ONE_ENTRY_BYTES.to_be_bytes());
	bytes.extend_from_slice(&u32::try_from(resource.len()).unwrap().to_be_bytes());
	bytes.extend_from_slice(resource);
	bytes
}

fn blank_native(fork: Option<&[u8]>) -> Attributes {
	Attributes {
		finder_info: None,
		resource_fork: fork.map(<[u8]>::to_vec),
		extended: Vec::new(),
		accounted_bytes: 0,
	}
}

fn prepare(
	base: &Path,
	sidecar: &str,
	extra_content: &str,
) -> Result<PreparedBuild, planning::Error> {
	let project = Project::parse(&format!(
		"schema_version=1\n[image]\nname='Fixture'\noutput='test.dmg'\nformat='udro'\n[metadata]\nsidecar='{sidecar}'\n[[contents]]\nsource='{SOURCE_NAME}'\ndestination='/renamed'\n{extra_content}"
	))
	.unwrap();
	let capabilities = BuildCapabilities {
		hfsplus_ascii: Availability::Available,
		metadata_sidecar: Availability::Available,
		..BuildCapabilities::default()
	};
	planning::prepare(
		&project,
		base,
		PreflightOptions {
			capabilities,
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: NonZeroUsize::MIN,
		},
		Cancellation::default(),
	)
}

#[test]
fn bound_sidecar_fills_missing_metadata_and_refuses_a_changed_snapshot() {
	let base = tempfile::tempdir().unwrap();
	fs::write(base.path().join(SOURCE_NAME), b"original data").unwrap();
	fs::write(base.path().join(SIDECAR_NAME), classic(RESOURCE)).unwrap();
	let plan = prepare(base.path(), SIDECAR_NAME, "").unwrap();
	let [entry] = plan.manifest().entries() else {
		panic!("expected one data entry")
	};
	assert_eq!(entry.destination.as_str(), "/renamed");
	let snapshot = plan
		.metadata_sidecar_for(entry.source.as_ref().unwrap())
		.unwrap();
	let mut remaining = TRANSFER_BUDGET;
	let AttributeCapture::Known(result) = merge(
		AttributeCapture::Known(blank_native(None)),
		snapshot,
		&mut remaining,
	)
	.unwrap() else {
		panic!("expected merged attributes")
	};
	assert_eq!(result.resource_fork.as_deref(), Some(RESOURCE));
	assert_eq!(remaining, TRANSFER_BUDGET - RESOURCE.len());
	fs::rename(base.path().join(SIDECAR_NAME), base.path().join(OTHER_NAME)).unwrap();
	fs::write(base.path().join(SIDECAR_NAME), classic(RESOURCE)).unwrap();
	assert!(matches!(
		merge(
			AttributeCapture::Known(blank_native(None)),
			snapshot,
			&mut remaining
		),
		Err(Error::Manifest(_))
	));
	assert_eq!(remaining, TRANSFER_BUDGET - RESOURCE.len());
}

#[test]
fn rejects_a_wrong_companion_a_sidecar_as_content_and_a_symlink() {
	let base = tempfile::tempdir().unwrap();
	fs::write(base.path().join(SOURCE_NAME), b"original data").unwrap();
	fs::write(base.path().join(OTHER_NAME), classic(RESOURCE)).unwrap();
	assert!(matches!(
		prepare(base.path(), OTHER_NAME, ""),
		Err(planning::Error::MetadataSidecar(Error::Target))
	));
	symlink(OTHER_NAME, base.path().join(SIDECAR_NAME)).unwrap();
	assert!(matches!(
		prepare(base.path(), SIDECAR_NAME, ""),
		Err(planning::Error::MetadataSidecar(Error::Manifest(_)))
	));
	fs::remove_file(base.path().join(SIDECAR_NAME)).unwrap();
	fs::write(base.path().join(SIDECAR_NAME), classic(RESOURCE)).unwrap();
	let added = format!("[[contents]]\nsource='{SIDECAR_NAME}'\ndestination='/explicit'\n");
	assert!(matches!(
		prepare(base.path(), SIDECAR_NAME, &added),
		Err(planning::Error::MetadataSidecar(Error::Included))
	));
}

#[test]
fn rejects_fork_conflicts_bad_ranges_and_insufficient_working_room() {
	let mut bytes = classic(RESOURCE);
	let sidecar = appledouble::parse(&bytes, TRANSFER_BUDGET).unwrap();
	assert!(matches!(
		merge_attributes(
			blank_native(Some(NATIVE_CONFLICT)),
			sidecar,
			TRANSFER_BUDGET
		),
		Err(Error::Conflict(Field::ResourceFork))
	));
	let sidecar = appledouble::parse(&bytes, TRANSFER_BUDGET).unwrap();
	let (unchanged, added) = merge_attributes(blank_native(Some(RESOURCE)), sidecar, 0).unwrap();
	assert_eq!(unchanged.resource_fork.as_deref(), Some(RESOURCE));
	assert_eq!(added, 0);
	let sidecar = appledouble::parse(&bytes, TRANSFER_BUDGET).unwrap();
	assert!(matches!(
		merge_attributes(blank_native(None), sidecar, RESOURCE.len() - 1),
		Err(Error::Budget)
	));
	bytes[RECORD_START_FIELD..][..size_of::<u32>()].copy_from_slice(&u32::MAX.to_be_bytes());
	assert!(matches!(
		appledouble::parse(&bytes, TRANSFER_BUDGET),
		Err(Error::Malformed(_))
	));
}

#[cfg(target_os = "macos")]
#[test]
fn accepts_native_ditto_and_reports_its_resource_fork_conflict() {
	use std::process::Command;
	const NATIVE_FORK: &str = "..namedfork/rsrc";
	const ATTRIBUTE: &str = "org.dmgez.sidecar.test";
	const ATTRIBUTE_VALUE: &str = "exact extended metadata";
	const FINDER_INFO: &str = "com.apple.FinderInfo";
	const FINDER_HEX: &str = "5445585474747874000000000000000000000000000000000000000000000000";
	const FINDER_TYPE_AND_CREATOR: &[u8] = b"TEXTttxt";
	const ARCHIVE_ENTRY: &str = "__MACOSX/._manual.txt";
	let base = tempfile::tempdir().unwrap();
	let source = base.path().join(SOURCE_NAME);
	let archive = base.path().join("metadata.zip");
	fs::write(&source, b"sample contents").unwrap();
	fs::write(source.join(NATIVE_FORK), RESOURCE).unwrap();
	assert!(
		Command::new("/usr/bin/xattr")
			.args(["-wx", FINDER_INFO, FINDER_HEX])
			.arg(&source)
			.status()
			.unwrap()
			.success()
	);
	assert!(
		Command::new("/usr/bin/xattr")
			.args(["-w", ATTRIBUTE, ATTRIBUTE_VALUE])
			.arg(&source)
			.status()
			.unwrap()
			.success()
	);
	assert!(
		Command::new("/usr/bin/ditto")
			.args(["-c", "-k", "--sequesterRsrc"])
			.arg(&source)
			.arg(&archive)
			.status()
			.unwrap()
			.success()
	);
	let output = Command::new("/usr/bin/unzip")
		.arg("-p")
		.arg(&archive)
		.arg(ARCHIVE_ENTRY)
		.output()
		.unwrap();
	assert!(output.status.success());
	let data = appledouble::parse(&output.stdout, TRANSFER_BUDGET).unwrap();
	assert_eq!(data.resource_fork, Some(RESOURCE));
	assert!(
		data.finder_info
			.unwrap()
			.starts_with(FINDER_TYPE_AND_CREATOR)
	);
	assert!(data.extended.iter().any(
		|entry| entry.name == ATTRIBUTE.as_bytes() && entry.value == ATTRIBUTE_VALUE.as_bytes()
	));
	assert!(matches!(
		merge_attributes(blank_native(Some(NATIVE_CONFLICT)), data, TRANSFER_BUDGET),
		Err(Error::Conflict(Field::ResourceFork))
	));
}
