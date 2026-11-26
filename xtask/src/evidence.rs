use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt::Write as _,
	fs::{self, File},
	io::{BufRead, BufReader},
	path::{Component, Path, PathBuf},
	process::{Command, Stdio},
};

use clap::Args;
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;
mod transcript;

const POLICY: &str = "verification/release-profile.json";
const OBLIGATIONS: &str = "verification/obligations.json";
const VERSION: u32 = 1;
const STRETCH: &str = "stretch";
const PASS: &str = "pass";
const UDIF_MANIFEST: &str = "crates/formats/dmgez-udif/Cargo.toml";

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type Names = BTreeSet<String>;

#[derive(Args)]
pub(super) struct Arguments {
	/// Clean Git checkout of the candidate.
	#[arg(long, default_value = ".")]
	root: PathBuf,
	/// JSON array of evidence records from the candidate runs.
	#[arg(long)]
	bundle: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
	milestone: String,
	profiles: BTreeMap<String, Profile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
	obligations: Names,
	method: String,
	features: Names,
	#[serde(default)]
	command: Option<Vec<String>>,
	check: transcript::Check,
}

#[derive(Deserialize)]
struct Registry {
	obligations: Vec<Obligation>,
}

#[derive(Deserialize)]
struct Obligation {
	id: String,
	activation: String,
	mandatory_when_activated: bool,
	required_methods: Names,
}

#[derive(Deserialize)]
struct Record {
	schema_version: u32,
	obligation_id: String,
	method: String,
	status: String,
	commit: String,
	dirty: bool,
	source_hash: String,
	profile: String,
	features: Vec<String>,
	tool_versions: BTreeMap<String, String>,
	command: Vec<String>,
	bounds: BTreeMap<String, serde_json::Value>,
	artifacts: Vec<Artifact>,
	runner: Runner,
	duration_ms: u64,
	exit_code: Option<i32>,
	proof: Option<Proof>,
}

#[derive(Deserialize)]
struct Runner {
	os: String,
	arch: String,
}

#[derive(Deserialize)]
struct Proof {
	theorem: String,
}

#[derive(Deserialize)]
struct Artifact {
	name: String,
	sha256: String,
	uri: PathBuf,
}

pub(super) fn check(arguments: Arguments) -> Result<()> {
	let root = arguments.root.canonicalize()?;
	if !git(
		&root,
		&["status", "--porcelain", "--untracked-files=normal"],
	)?
	.is_empty()
	{
		return Err("candidate checkout has tracked or untracked changes".into());
	}
	let commit = git(&root, &["rev-parse", "HEAD"])?;
	let commit = std::str::from_utf8(&commit)?.trim();
	let digest = archive_digest(&root, commit)?;
	let policy: Policy = read_json(&root.join(POLICY))?;
	let registry: Registry = read_json(&root.join(OBLIGATIONS))?;
	let bundle = arguments.bundle.canonicalize()?;
	let records: Vec<Record> = read_json(&bundle)?;
	let artifacts = bundle
		.parent()
		.ok_or("evidence bundle has no parent directory")?;
	let count = validate(&policy, registry, records, commit, &digest, artifacts)?;
	println!(
		"Verified {count} required evidence records for {} at {commit}",
		policy.milestone,
	);
	Ok(())
}

fn validate(
	policy: &Policy,
	registry: Registry,
	records: Vec<Record>,
	commit: &str,
	digest: &str,
	artifacts: &Path,
) -> Result<usize> {
	let milestone = cargo_metadata::semver::Version::parse(&policy.milestone)?;
	let obligations: BTreeMap<_, _> = registry
		.obligations
		.into_iter()
		.filter(|entry| entry.mandatory_when_activated && entry.activation != STRETCH)
		.map(|entry| {
			Ok((
				cargo_metadata::semver::Version::parse(&entry.activation)?,
				entry.id,
				entry.required_methods,
			))
		})
		.collect::<Result<Vec<_>>>()?
		.into_iter()
		.filter(|(required, _, _)| *required <= milestone)
		.map(|(_, id, methods)| (id, methods))
		.collect();
	if obligations.is_empty() {
		return Err("release has no active verification obligations".into());
	}

	let mut missing = BTreeSet::new();
	for (profile_name, profile) in &policy.profiles {
		if profile.obligations.is_empty() {
			return Err(format!("profile {profile_name} has no obligations").into());
		}
		for id in &profile.obligations {
			if !obligations.contains_key(id) {
				return Err(format!("{profile_name} names inactive obligation {id}").into());
			}
			missing.insert((profile_name.as_str(), id.as_str()));
		}
	}
	for (id, methods) in &obligations {
		for method in methods {
			if !policy
				.profiles
				.values()
				.any(|profile| profile.method == *method && profile.obligations.contains(id))
			{
				return Err(format!("release policy omits {id} method {method}").into());
			}
		}
	}

	let count = missing.len();
	for record in &records {
		let label = format!("{} / {}", record.obligation_id, record.profile);
		let profile = policy
			.profiles
			.get(&record.profile)
			.ok_or_else(|| format!("unknown evidence profile {label}"))?;
		if !missing.remove(&(record.profile.as_str(), record.obligation_id.as_str())) {
			return Err(format!("duplicate or unexpected evidence for {label}").into());
		}
		validate_record(record, profile, commit, digest, artifacts)
			.map_err(|error| format!("{label}: {error}"))?;
	}
	if let Some((profile, obligation)) = missing.first() {
		return Err(format!("missing evidence for {obligation} / {profile}").into());
	}
	Ok(count)
}

fn validate_record(
	record: &Record,
	profile: &Profile,
	commit: &str,
	digest: &str,
	root: &Path,
) -> Result<()> {
	if record.schema_version != VERSION
		|| record.status != PASS
		|| record.dirty
		|| record.exit_code != Some(0)
	{
		return Err("evidence must use schema v1 and record a clean successful invocation".into());
	}
	if record.commit != commit || record.source_hash != digest {
		return Err("evidence source differs from the candidate Git archive".into());
	}
	let features: Names = record.features.iter().cloned().collect();
	if record.method != profile.method
		|| features != profile.features
		|| features.len() != record.features.len()
		|| profile
			.command
			.as_ref()
			.is_some_and(|command| *command != record.command)
	{
		return Err("verification method or feature profile differs from policy".into());
	}
	if record.duration_ms == 0
		|| record.tool_versions.is_empty()
		|| record.command.is_empty()
		|| record.runner.os.trim().is_empty()
		|| record.runner.arch.trim().is_empty()
	{
		return Err("incomplete invocation provenance".into());
	}
	if let transcript::Check::Kani { filter, .. } = &profile.check
		&& (record.bounds.is_empty()
			|| record
				.proof
				.as_ref()
				.is_none_or(|proof| proof.theorem != *filter)
			|| !record.tool_versions.contains_key("kani")
			|| !kani_command(record, filter, &features))
	{
		return Err("Kani proof metadata is missing or has a different harness filter".into());
	}
	let [transcript] = record.artifacts.as_slice() else {
		return Err("release profiles require exactly one invocation transcript".into());
	};
	if transcript.name.trim().is_empty() {
		return Err("invocation transcript has no name".into());
	}
	let bytes = read_artifact(root, transcript)?;
	profile.check.verify(std::str::from_utf8(&bytes)?)
}

fn kani_command(record: &Record, filter: &str, features: &Names) -> bool {
	let mut expected = vec![
		"cargo",
		"kani",
		"--manifest-path",
		UDIF_MANIFEST,
		"--no-default-features",
	];
	let features = features
		.iter()
		.map(String::as_str)
		.collect::<Vec<_>>()
		.join(",");
	if !features.is_empty() {
		expected.extend(["--features", &features]);
	}
	expected.extend(["--harness", filter, "--output-format", "terse"]);
	record.command == expected
}

fn read_artifact(root: &Path, artifact: &Artifact) -> Result<Vec<u8>> {
	if artifact.uri.as_os_str().is_empty()
		|| !artifact
			.uri
			.components()
			.all(|part| matches!(part, Component::Normal(_)))
	{
		return Err("artifact path must be relative to the evidence bundle".into());
	}
	let path = root.join(&artifact.uri).canonicalize()?;
	if !path.starts_with(root) || !path.is_file() {
		return Err("artifact is outside the bundle directory or is not a file".into());
	}
	let bytes = fs::read(path)?;
	if hex(&Sha256::digest(&bytes)) != artifact.sha256 {
		return Err("artifact SHA-256 mismatch".into());
	}
	Ok(bytes)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
	Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
	let output = Command::new("git").current_dir(root).args(args).output()?;
	if !output.status.success() {
		return Err(format!(
			"git {} failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&output.stderr)
		)
		.into());
	}
	Ok(output.stdout)
}

fn archive_digest(root: &Path, commit: &str) -> Result<String> {
	let mut child = Command::new("git")
		.current_dir(root)
		.args(["-c", "tar.umask=0002", "archive", "--format=tar", commit])
		.stdout(Stdio::piped())
		.spawn()?;
	let result = hash_reader(BufReader::new(
		child.stdout.take().ok_or("missing Git archive pipe")?,
	));
	let status = child.wait()?;
	if !status.success() {
		return Err("could not archive the release candidate".into());
	}
	result
}

fn hash_reader(mut input: impl BufRead) -> Result<String> {
	let mut hash = Sha256::new();
	loop {
		let bytes = input.fill_buf()?;
		if bytes.is_empty() {
			return Ok(hex(&hash.finalize()));
		}
		hash.update(bytes);
		let bytes = bytes.len();
		input.consume(bytes);
	}
}

fn hex(bytes: &[u8]) -> String {
	let mut output = String::with_capacity(bytes.len() * 2);
	for byte in bytes {
		write!(output, "{byte:02x}").expect("writing to String cannot fail");
	}
	output
}
