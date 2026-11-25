use std::{
	borrow::Cow,
	cell::RefCell,
	fs,
	io::{self, Read, Write},
	path::Path,
};

use dmgez_config::project_edit::{MAX_EDIT_BATCH_BYTES, ProjectEdit, parse_edit_batch};
use dmgez_core::output::{StagedReader, publish_new};
use dmgez_hfsplus_read::AlternateStatus;
use dmgez_hfsplus_read::browse::{Limits, with_catalog};
use dmgez_operations::edit::image_source::{
	ImageRevision, ImageSource, ImageSourceOptions, SourceEnvelope,
};
use dmgez_udif::writer::{RebuildMetadata, WriteOptions, WriteReport, write_rebuilt_image};
use dmgez_vfs::{Disk, Filesystem, Layout, VolumeId};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
	CliResult,
	edit_input::{SHA256_BYTES, SHA256_HEX_BYTES, Snapshot, encode_sha256},
	report_output::{EscapedText, write_report},
	staged_image::{invalid, open_verified_stage},
};

use super::Arguments;

const SCHEMA_VERSION: u16 = 1;
const SHA256_FORMAT: &str = "expected image SHA-256 must contain 64 hexadecimal digits";
const OUTPUT_EXISTS: &str = "image edit destination already exists";
const OUTPUT_FILENAME: &str = "the image edit destination has no file name";
const OUTPUT_DIRECTORY: &str = "the image edit destination parent is not a directory";
const CHANGES_DIRECTORY: &str = "the edit batch has no parent directory";
const MISSING_ENCODED_STAGE: &str = "the staged rebuild has no encoding report";
const METADATA_MISMATCH: &str = "the staged image differs from its admitted container metadata";
const LOGICAL_MISMATCH: &str = "the staged image does not contain the sealed logical bytes";
const STAGED_LAYOUT: &str = "the rebuilt HFS+ volume does not cover the whole staged logical disk";
const STAGED_STRUCTURE: &str = "the rebuilt HFS+ filesystem has incomplete structural coverage";

pub(super) fn run(arguments: &Arguments) -> CliResult {
	let expected = arguments
		.expect_image_sha256
		.as_deref()
		.map(parse_sha256)
		.transpose()?;
	check_destination(&arguments.output)?;
	let changes = Snapshot::open(&arguments.changes, MAX_EDIT_BATCH_BYTES)?;
	let changes_dir = changes.path.parent().ok_or(CHANGES_DIRECTORY)?;
	let edits = parse_edit_batch(&changes.text)?;
	let mut source = ImageSource::open(&arguments.image, expected, ImageSourceOptions::default())?;
	let revision = source.revision();
	let source_envelope = match source.envelope() {
		SourceEnvelope::Unsigned => "unsigned",
		SourceEnvelope::SignedDigestsVerified => "signed_digests_verified",
		_ => return Err("the image uses an unsupported source envelope".into()),
	};
	let rebuilt = if arguments.dry_run {
		let plan = rebuild(&mut source, &edits, changes_dir, None)?;
		changes.validate()?;
		source.revalidate()?;
		plan
	} else {
		let source = RefCell::new(source);
		publish_new(
			&arguments.output,
			|sink| {
				let mut source = source.try_borrow_mut().map_err(io::Error::other)?;
				rebuild(&mut source, &edits, changes_dir, Some(sink))
			},
			|stage, rebuilt| {
				verify(stage, rebuilt)?;
				changes.validate()?;
				source
					.try_borrow()
					.map_err(io::Error::other)?
					.revalidate()
					.map_err(io::Error::other)
			},
		)?
	};
	let encoded = rebuilt.encoded.as_ref();
	let report = Report {
		schema_version: SCHEMA_VERSION,
		image_sha256: encode_sha256(revision.sha256()),
		source_envelope,
		output: arguments.output.to_string_lossy(),
		edits: edits.len(),
		entries: rebuilt.entries,
		logical_bytes: rebuilt.logical_bytes,
		physical_bytes: encoded.map(|value| value.written.physical_bytes),
		logical_sha256: encoded.map(|value| encode_sha256(&value.logical_sha256)),
		published: encoded.is_some(),
		verified: encoded.is_some(),
		trust: "unsigned",
	};
	report.write(arguments.json)
}

fn check_destination(path: &Path) -> io::Result<()> {
	if path.file_name().is_none() {
		return Err(io::Error::new(io::ErrorKind::InvalidInput, OUTPUT_FILENAME));
	}
	let parent = path
		.parent()
		.filter(|candidate| !candidate.as_os_str().is_empty())
		.unwrap_or_else(|| Path::new("."));
	if !fs::metadata(parent)?.is_dir() {
		return Err(io::Error::new(
			io::ErrorKind::NotADirectory,
			OUTPUT_DIRECTORY,
		));
	}
	match fs::symlink_metadata(path) {
		Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, OUTPUT_EXISTS)),
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
		Err(error) => Err(error),
	}
}

fn parse_sha256(value: &str) -> CliResult<ImageRevision> {
	if value.len() != SHA256_HEX_BYTES || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(SHA256_FORMAT.into());
	}
	let mut digest = [0; SHA256_BYTES];
	for (index, byte) in digest.iter_mut().enumerate() {
		let start = index * 2;
		*byte = u8::from_str_radix(&value[start..start + 2], 16)?;
	}
	Ok(ImageRevision::from_sha256(digest))
}

struct Rebuilt {
	entries: usize,
	logical_bytes: u64,
	encoded: Option<Encoded>,
}

struct Encoded {
	written: WriteReport,
	metadata: RebuildMetadata,
	logical_sha256: [u8; SHA256_BYTES],
}

fn rebuild(
	source: &mut ImageSource,
	edits: &[ProjectEdit],
	changes_dir: &Path,
	sink: Option<&mut dyn Write>,
) -> io::Result<Rebuilt> {
	let source_metadata = source.container_metadata();
	let mut disk = Disk::discover(source, dmgez_vfs::Limits::default()).map_err(invalid)?;
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		Limits::default(),
		|volume, catalog| {
			let plan = dmgez_hfsplus::rebuild::seal(volume, &catalog, edits, changes_dir)
				.map_err(invalid)?;
			let logical_bytes = plan.logical_bytes();
			let options = WriteOptions {
				profile: source_metadata.source_profile(),
				..WriteOptions::default()
			};
			options.max_data_runs(logical_bytes).map_err(invalid)?;
			let encoded = if let Some(sink) = sink {
				let mut digest = Sha256::new();
				let reader = HashReader {
					reader: plan.reader(),
					digest: &mut digest,
				};
				let metadata = source_metadata.with_logical_bytes(logical_bytes);
				let written =
					write_rebuilt_image(reader, sink, &options, metadata).map_err(invalid)?;
				Some(Encoded {
					written,
					metadata,
					logical_sha256: digest.finalize().into(),
				})
			} else {
				None
			};
			Ok(Rebuilt {
				entries: plan.entries(),
				logical_bytes,
				encoded,
			})
		},
	)
	.map_err(invalid)?
}

fn verify(stage: StagedReader<'_>, rebuilt: &Rebuilt) -> io::Result<()> {
	let encoded = rebuilt
		.encoded
		.as_ref()
		.ok_or_else(|| invalid(MISSING_ENCODED_STAGE))?;
	let mut image = open_verified_stage(stage, &encoded.written)?;
	let actual = image.rebuild_metadata().map_err(invalid)?;
	if !encoded.metadata.matches_preserved_fields(&actual)
		|| encoded.metadata.source_profile() != actual.source_profile()
	{
		return Err(invalid(METADATA_MISMATCH));
	}
	let mut sink = HashSink(Sha256::new());
	image.write_raw_to(&mut sink).map_err(invalid)?;
	let actual_digest: [u8; SHA256_BYTES] = sink.0.finalize().into();
	if actual_digest != encoded.logical_sha256 {
		return Err(invalid(LOGICAL_MISMATCH));
	}
	let mut disk = Disk::discover(image, dmgez_vfs::Limits::default()).map_err(invalid)?;
	if !matches!(
		disk.layout(),
		Layout::Partitionless { filesystem: Filesystem::HfsPlus, range }
			if *range == (0..rebuilt.logical_bytes)
	) {
		return Err(invalid(STAGED_LAYOUT));
	}
	with_catalog(
		&mut disk,
		VolumeId::WholeDisk,
		Limits::default(),
		|volume, _catalog| {
			let report = volume
				.check_structure(Default::default())
				.map_err(invalid)?;
			if report.alternate != AlternateStatus::MatchesPrimary
				|| report.unclaimed_allocated_blocks != 0
				|| report.unexamined_private_files != 0
				|| report.unsupported_directory_links != 0
			{
				return Err(invalid(STAGED_STRUCTURE));
			}
			Ok(())
		},
	)
	.map_err(invalid)?
}

struct HashReader<'digest, R> {
	reader: R,
	digest: &'digest mut Sha256,
}

impl<R: Read> Read for HashReader<'_, R> {
	fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
		let received = self.reader.read(buffer)?;
		self.digest.update(&buffer[..received]);
		Ok(received)
	}
}

struct HashSink(Sha256);

impl Write for HashSink {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.0.update(bytes);
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[derive(Serialize)]
struct Report<'path> {
	schema_version: u16,
	image_sha256: String,
	source_envelope: &'static str,
	output: Cow<'path, str>,
	edits: usize,
	entries: usize,
	logical_bytes: u64,
	physical_bytes: Option<u64>,
	logical_sha256: Option<String>,
	published: bool,
	verified: bool,
	trust: &'static str,
}

impl Report<'_> {
	fn write(&self, json: bool) -> CliResult {
		write_report(json, self, |output| {
			writeln!(output, "image_sha256 = {}", self.image_sha256)?;
			writeln!(output, "source_envelope = {}", self.source_envelope)?;
			writeln!(output, "output = {}", EscapedText(&self.output))?;
			writeln!(output, "edits = {}", self.edits)?;
			writeln!(output, "entries = {}", self.entries)?;
			writeln!(output, "logical_bytes = {}", self.logical_bytes)?;
			if let Some(bytes) = self.physical_bytes {
				writeln!(output, "physical_bytes = {bytes}")?;
			}
			if let Some(digest) = &self.logical_sha256 {
				writeln!(output, "logical_sha256 = {digest}")?;
			}
			writeln!(output, "published = {}", self.published)?;
			writeln!(output, "verified = {}", self.verified)?;
			writeln!(output, "trust = {}", self.trust)
		})
	}
}
