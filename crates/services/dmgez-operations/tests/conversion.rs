//! D-OPS-01 evidence for container-only conversion.

#![cfg(feature = "convert")]

use std::{
	cell::Cell,
	fs,
	io::{self, Write},
};

use dmgez_operations::convert::{self, Error, FileError, Options, OutputCheck, TrustState};
use dmgez_udif::{
	Error as UdifError,
	reader::{CheckStatus, Image, Limits, ReadAt},
	writer::{Profile, WriteOptions},
};
use plist::Value;

const UDRO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDRO.dmg");
const UDZO: &[u8] = include_bytes!("../../../../tests/fixtures/images/tiny-UDZO.dmg");
const RAW: &[u8] = include_bytes!("../../../../tests/fixtures/expected/tiny.raw");
const COMMENT: &[u8] = include_bytes!("../../../../tests/fixtures/images/comment-run.dmg");
const HYBRID: &[u8] = include_bytes!("../../../../tests/fixtures/images/hybrid-UDZO.dmg");
const ULFO: &[u8] = include_bytes!("../../../formats/dmgez-udif/tests/fixtures/apple-ulfo.dmg");
const TRAILER_BYTES: usize = 512;
const VARIANT_FIELD: usize = 488;
const DEVICE_VARIANT: u32 = 1;
const SIGNATURE_FIELD: usize = 232;
const UNUSED_CHECKSUM_PAYLOAD: usize = 92;
const XML_START_FIELD: std::ops::Range<usize> = 216..224;
const XML_LENGTH_FIELD: std::ops::Range<usize> = 224..232;

fn options(profile: Profile) -> Options {
	Options {
		output: WriteOptions {
			profile,
			..WriteOptions::default()
		},
		..Options::default()
	}
}

#[test]
fn rewrites_both_native_profiles_and_preserves_geometry_and_resources() {
	for (source, from, to) in [
		(UDRO, Profile::Udro, Profile::Udzo),
		(UDZO, Profile::Udzo, Profile::Udro),
	] {
		let input = Image::open(source, Limits::default()).unwrap();
		let mut stored = Vec::new();
		let report = convert::stream(source, &mut stored, &options(to)).unwrap();
		assert_eq!(report.source_profile, from);
		assert_eq!(report.output_profile, to);
		assert_eq!(report.trust, TrustState::UnsignedPreserved);
		assert_eq!(report.output_check, OutputCheck::Writer);
		assert_eq!(report.input_checks.data_fork, CheckStatus::Passed);
		let mut output = Image::open(stored.as_slice(), Limits::default()).unwrap();
		assert!(
			input
				.rebuild_metadata()
				.unwrap()
				.matches_preserved_fields(&output.rebuild_metadata().unwrap())
		);
		assert_eq!(output.rebuild_metadata().unwrap().source_profile(), to);
		let original_resources = input.descriptor().resources.as_dictionary().unwrap();
		let rewritten_resources = output.descriptor().resources.as_dictionary().unwrap();
		assert_eq!(
			original_resources["resource-fork"].as_dictionary().unwrap()["plst"],
			rewritten_resources["resource-fork"]
				.as_dictionary()
				.unwrap()["plst"]
		);
		output.verify().unwrap();
		let mut bytes = Vec::new();
		output.write_raw_to(&mut bytes).unwrap();
		assert_eq!(bytes, RAW);
	}
}

#[test]
fn keeps_the_admitted_device_variant() {
	let mut device = UDRO.to_vec();
	let offset = device.len() - TRAILER_BYTES + VARIANT_FIELD;
	device[offset..offset + size_of::<u32>()].copy_from_slice(&DEVICE_VARIANT.to_be_bytes());
	let mut data = Vec::new();
	convert::stream(device.as_slice(), &mut data, &options(Profile::Udzo)).unwrap();
	assert_eq!(
		Image::open(data.as_slice(), Limits::default())
			.unwrap()
			.descriptor()
			.variant,
		DEVICE_VARIANT
	);
}

#[test]
fn rejects_opaque_metadata_and_corruption_before_writing() {
	let mut signed = UDRO.to_vec();
	signed.last_chunk_mut::<TRAILER_BYTES>().unwrap()[SIGNATURE_FIELD] = 1;
	let mut extended_checksum = UDRO.to_vec();
	extended_checksum.last_chunk_mut::<TRAILER_BYTES>().unwrap()[UNUSED_CHECKSUM_PAYLOAD] = 1;
	let extra = add_resource(UDRO);
	let gap = insert_unmapped_byte(UDRO);
	for source in [
		&signed,
		&extended_checksum,
		&extra,
		&gap,
		COMMENT,
		HYBRID,
		ULFO,
	] {
		let mut data = Vec::new();
		assert!(matches!(
			convert::stream(source, &mut data, &options(Profile::Udro)),
			Err(Error::Input(UdifError::Unsupported(_)))
		));
		assert!(data.is_empty());
	}
	let mut damaged = UDRO.to_vec();
	damaged[0] ^= 1;
	let mut data = Vec::new();
	assert!(matches!(
		convert::stream(damaged.as_slice(), &mut data, &options(Profile::Udro)),
		Err(Error::Input(UdifError::ChecksumMismatch { .. }))
	));
	assert!(data.is_empty());
}

fn add_resource(image: &[u8]) -> Vec<u8> {
	let mut trailer = *image.last_chunk::<TRAILER_BYTES>().unwrap();
	let xml_start = u64::from_be_bytes(trailer[XML_START_FIELD].try_into().unwrap()) as usize;
	let mut resources =
		Value::from_reader_xml(&image[xml_start..image.len() - TRAILER_BYTES]).unwrap();
	resources
		.as_dictionary_mut()
		.unwrap()
		.insert("opaque-test-resource".into(), Value::Data(vec![1]));
	let mut data = image[..xml_start].to_vec();
	resources.to_writer_xml(&mut data).unwrap();
	trailer[XML_LENGTH_FIELD].copy_from_slice(&((data.len() - xml_start) as u64).to_be_bytes());
	data.extend_from_slice(&trailer);
	data
}

fn insert_unmapped_byte(image: &[u8]) -> Vec<u8> {
	let mut trailer = *image.last_chunk::<TRAILER_BYTES>().unwrap();
	let xml_start = u64::from_be_bytes(trailer[XML_START_FIELD].try_into().unwrap());
	let mut data = image[..xml_start as usize].to_vec();
	data.push(1);
	data.extend_from_slice(&image[xml_start as usize..image.len() - TRAILER_BYTES]);
	trailer[XML_START_FIELD].copy_from_slice(&(xml_start + 1).to_be_bytes());
	data.extend_from_slice(&trailer);
	data
}

#[test]
fn stages_exact_file_output_and_refuses_existing_paths_or_aliases() {
	let tmp = tempfile::tempdir().unwrap();
	let source = tmp.path().join("source.dmg");
	let output = tmp.path().join("output.dmg");
	fs::write(&source, UDZO).unwrap();
	assert!(matches!(
		convert::file(&source, &source, &options(Profile::Udro)),
		Err(FileError::DestinationExists)
	));
	fs::hard_link(&source, &output).unwrap();
	assert!(matches!(
		convert::file(&source, &output, &options(Profile::Udro)),
		Err(FileError::DestinationExists)
	));
	assert_eq!(fs::read(&output).unwrap(), UDZO);
	fs::remove_file(&output).unwrap();
	let report = convert::file(&source, &output, &options(Profile::Udro)).unwrap();
	assert_eq!(report.output_check, OutputCheck::VerifiedAndCompared);
	assert_eq!(
		fs::metadata(output).unwrap().len(),
		report.output.physical_bytes
	);
	assert_eq!(fs::read(source).unwrap(), UDZO);
	assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 2);
}

#[test]
fn a_source_changed_during_emission_never_returns_success() {
	struct Source<'state>(&'state Cell<bool>);
	impl ReadAt for Source<'_> {
		fn len(&self) -> u64 {
			UDRO.len() as u64
		}
		fn check_unchanged(&self) -> Result<(), UdifError> {
			if self.0.get() {
				return Err(UdifError::SourceChanged);
			}
			Ok(())
		}
		fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<(), UdifError> {
			UDRO.read_exact_at(offset, output)
		}
	}
	struct Trigger<'state>(&'state Cell<bool>);
	impl Write for Trigger<'_> {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			self.0.set(true);
			Ok(bytes.len())
		}
		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}
	let changed = Cell::new(false);
	assert!(convert::stream(Source(&changed), Trigger(&changed), &options(Profile::Udzo)).is_err());
	assert!(changed.get());
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires Apple hdiutil"]
fn apple_verifies_and_exports_both_directions() {
	use std::process::Command;

	const HDIUTIL: &str = "/usr/bin/hdiutil";
	let tmp = tempfile::tempdir().unwrap();
	for (index, (input, target)) in [(UDRO, Profile::Udzo), (UDZO, Profile::Udro)]
		.into_iter()
		.enumerate()
	{
		let source = tmp.path().join(format!("source-{index}.dmg"));
		let output = tmp.path().join(format!("output-{index}.dmg"));
		let raw = tmp.path().join(format!("export-{index}.cdr"));
		fs::write(&source, input).unwrap();
		convert::file(&source, &output, &options(target)).unwrap();
		assert!(
			Command::new(HDIUTIL)
				.arg("verify")
				.arg(&output)
				.status()
				.unwrap()
				.success()
		);
		assert!(
			Command::new(HDIUTIL)
				.arg("convert")
				.arg(&output)
				.args(["-format", "UDTO", "-o"])
				.arg(&raw)
				.status()
				.unwrap()
				.success()
		);
		assert_eq!(fs::read(raw).unwrap(), RAW);
		assert_eq!(fs::read(source).unwrap(), input);
	}
}
