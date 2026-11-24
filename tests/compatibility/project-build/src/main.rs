use std::{
	env,
	error::Error,
	fs,
	io::{self, Read},
	path::PathBuf,
	thread,
};

use dmgez_builder::preflight::{OutputIntent, PreflightOptions, ReleaseIntent};
use dmgez_config::{MAX_MANIFEST_BYTES, Project};
use dmgez_hfs_adapter_probe::hfsplus;

const ARGUMENT_ERROR: &str = "expected exactly one TOML project path";
const MISSING_PARENT: &str = "the TOML path has no parent directory";

fn main() -> Result<(), Box<dyn Error>> {
	let mut arguments = env::args_os().skip(1);
	let path = arguments.next().map(PathBuf::from).ok_or(ARGUMENT_ERROR)?;
	if arguments.next().is_some() {
		return Err(ARGUMENT_ERROR.into());
	}
	let path = path.canonicalize()?;
	let directory = path.parent().ok_or(MISSING_PARENT)?;
	let mut source = String::new();
	fs::File::open(&path)?
		.take(MAX_MANIFEST_BYTES as u64 + 1)
		.read_to_string(&mut source)?;
	let project = Project::parse(&source)?;
	let prepared = hfsplus::prepare(
		&project,
		directory,
		PreflightOptions {
			capabilities: hfsplus::capabilities(),
			output: OutputIntent::Path,
			release: ReleaseIntent::Unsigned,
			host_threads: thread::available_parallelism()?,
		},
		Default::default(),
	)?;
	let sealed = prepared.seal()?;
	for loss in sealed.metadata_losses() {
		eprintln!("{loss:?}");
	}
	let report = sealed.publish(None)?;
	let physical_bytes = report.report.image.physical_bytes;
	let logical_bytes = report.report.image.logical_bytes;
	let metadata_losses = report
		.metadata_losses
		.iter()
		.map(|entry| entry.details.len())
		.sum::<usize>();
	io::Write::write_all(
		&mut io::stdout().lock(),
		format!("{logical_bytes} logical bytes, {physical_bytes} physical bytes, {metadata_losses} metadata losses\n").as_bytes(),
	)?;
	Ok(())
}
