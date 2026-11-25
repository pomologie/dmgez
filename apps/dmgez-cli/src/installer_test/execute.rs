use std::{
	fs::{self, File, Metadata},
	io,
	os::unix::fs::MetadataExt,
	path::{Path, absolute},
	process::ExitCode,
};

use dmgez_operations::install::{InstallerLimits, InstallerReport, check_installer};
use dmgez_udif::{
	Error as UdifError,
	reader::{CheckStatus, FileSource, Image, Limits, Verification},
};
use dmgez_vfs::volume::{VolumeBackend, VolumeView};

use crate::{CliResult, volume_commands::Filesystem};

use super::Arguments;

mod report;
mod security;

use report::{Check, Checksums, Report, Status};

pub(super) fn run(arguments: &Arguments) -> CliResult<ExitCode> {
	let path = absolute(&arguments.image)?;
	let mut report = Report::new(arguments, &path);
	if let Err(error) = inspect(&path, arguments, &mut report) {
		report.source = Check::detail(Status::Error, error);
	}
	report.write(arguments.json)?;
	Ok(report.exit_code())
}

fn inspect(path: &Path, arguments: &Arguments, report: &mut Report<'_>) -> CliResult {
	let source = Source::open(path)?;
	report.source = Check::new(Status::Passed);
	let opened = Image::open_release(
		FileSource::new(source.file.try_clone()?)?,
		Limits::default(),
	);
	let mut image = match opened {
		Ok(image) => image,
		Err(error) => {
			report.container = container_error(&error);
			return source.validate();
		}
	};
	report.container = match image.verify() {
		Ok(verification) => container_check(&verification),
		Err(error) => container_error(&error),
	};
	source.validate()?;
	if report.container.status != Status::Passed {
		return Ok(());
	}
	report.layout = match arguments.selection.filesystem() {
		Filesystem::Hfsplus => hfsplus(&mut image, arguments),
		Filesystem::Apfs => apfs(&mut image, arguments, report),
	};
	source.validate()?;
	if arguments.require_signature {
		report.signature = security::signature(path);
		source.validate()?;
	}
	if arguments.require_ticket {
		report.ticket = security::ticket(path);
		source.validate()?;
	}
	if arguments.require_native_assessment {
		report.native_assessment = security::assess(path);
		source.validate()?;
	}
	Ok(())
}

fn container_check(verification: &Verification) -> Check<Checksums> {
	let checksums = Checksums {
		data_fork_verified: verification.data_fork == CheckStatus::Passed,
		master_verified: verification.master == CheckStatus::Passed,
		tables: verification.tables.len(),
		verified_tables: verification
			.tables
			.iter()
			.filter(|&&status| status == CheckStatus::Passed)
			.count(),
	};
	let status = if checksums.data_fork_verified
		&& checksums.master_verified
		&& checksums.verified_tables == checksums.tables
	{
		Status::Passed
	} else {
		Status::Unavailable
	};
	Check::evidence(status, checksums)
}

fn container_error(error: &UdifError) -> Check<Checksums> {
	let status = match error {
		UdifError::InvalidFormat(_)
		| UdifError::Corrupt(_)
		| UdifError::ChecksumMismatch { .. }
		| UdifError::InvalidZlib { .. }
		| UdifError::InvalidLzfse { .. }
		| UdifError::InvalidLzma { .. }
		| UdifError::Resource(_) => Status::Failed,
		UdifError::Unsupported(_)
		| UdifError::UnsupportedRun { .. }
		| UdifError::FeatureNotCompiled(_)
		| UdifError::ResourceLimit(_) => Status::Unavailable,
		_ => Status::Error,
	};
	Check::detail(status, error)
}

fn hfsplus(image: &mut Image<FileSource>, arguments: &Arguments) -> Check<InstallerReport> {
	#[cfg(feature = "hfsplus-read")]
	{
		layout_result(crate::volume_input::with_hfs_disk(
			image,
			arguments.partition,
			|view, _| check_layout(view),
		))
	}
	#[cfg(not(feature = "hfsplus-read"))]
	{
		let _ = (image, arguments);
		Check::detail(
			Status::Unavailable,
			crate::volume_commands::HFSPLUS_READ_FEATURE,
		)
	}
}

fn apfs(
	image: &mut Image<FileSource>,
	arguments: &Arguments,
	report: &mut Report<'_>,
) -> Check<InstallerReport> {
	#[cfg(feature = "apfs-read")]
	{
		use crate::volume_input::{require_apfs_checkpoint, with_apfs_disk};
		layout_result((|| {
			let volume = arguments.selection.apfs_volume()?;
			let xid = arguments.selection.apfs_checkpoint();
			with_apfs_disk(image, arguments.partition, xid, |selected, container| {
				require_apfs_checkpoint(xid, container.diagnostics())?;
				let backend = container.open_volume(volume)?;
				report.apfs_volume = Some(crate::volume_commands::VolumeIdentity::from(
					&backend.info(),
				));
				check_layout(&mut VolumeView::new(selected, backend))
			})
		})())
	}
	#[cfg(not(feature = "apfs-read"))]
	{
		let _ = (image, arguments, report);
		Check::detail(
			Status::Unavailable,
			crate::volume_commands::APFS_READ_FEATURE,
		)
	}
}

fn check_layout<B: VolumeBackend>(view: &mut VolumeView<B>) -> CliResult<InstallerReport>
where
	B::Id: Eq + std::hash::Hash,
{
	Ok(check_installer(view, InstallerLimits::default(), || false)?)
}

fn layout_result(result: CliResult<InstallerReport>) -> Check<InstallerReport> {
	match result {
		Ok(report) => {
			let status = if report.is_valid() {
				Status::Passed
			} else {
				Status::Failed
			};
			Check::evidence(status, report)
		}
		Err(error) => Check::detail(Status::Error, error),
	}
}

struct Source<'path> {
	path: &'path Path,
	file: File,
	initial: Metadata,
}

impl<'path> Source<'path> {
	fn open(path: &'path Path) -> CliResult<Self> {
		let initial = fs::symlink_metadata(path)?;
		if !initial.is_file() {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"installer image must be a regular file",
			)
			.into());
		}
		let source = Self {
			path,
			file: File::open(path)?,
			initial,
		};
		source.validate()?;
		Ok(source)
	}

	fn validate(&self) -> CliResult {
		for observed in [self.file.metadata()?, fs::symlink_metadata(self.path)?] {
			if !same_file(&self.initial, &observed) {
				return Err(io::Error::new(
					io::ErrorKind::InvalidData,
					"installer image changed during testing",
				)
				.into());
			}
		}
		Ok(())
	}
}

fn same_file(expected: &Metadata, observed: &Metadata) -> bool {
	observed.is_file()
		&& expected.dev() == observed.dev()
		&& expected.ino() == observed.ino()
		&& expected.len() == observed.len()
		&& expected.mtime() == observed.mtime()
		&& expected.mtime_nsec() == observed.mtime_nsec()
		&& expected.ctime() == observed.ctime()
		&& expected.ctime_nsec() == observed.ctime_nsec()
}
