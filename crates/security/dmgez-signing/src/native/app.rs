use std::path::{Path, PathBuf};

use crate::{
	AppEntitlements, AppSigningItemReport, AppSigningReport, ArtifactKind, ArtifactProblem,
	PolicyAssessment, ResolvedBackend, SignError,
};

use super::{artifact::Artifact, identity, policy::AppIdentity, stage::Stage};

mod copy;
mod entitlements;
mod macho;
mod plan;

#[cfg(test)]
mod tests;

const REPORT_VERSION: u16 = 1;
const ROOT_ITEM: &str = ".";

pub(crate) fn sign(
	source: &Path,
	destination: &Path,
	fingerprint: &str,
	entitlements: &[AppEntitlements<'_>],
) -> Result<AppSigningReport, SignError> {
	let developer = AppIdentity::new(fingerprint)?;
	let source = open_app(source)?;
	let published = stage_and_sign(
		&source,
		destination,
		entitlements,
		|| {
			if identity::developer_id_available(developer.fingerprint())
				.map_err(|source| SignError::IdentityLookup(Box::new(source)))?
			{
				Ok(())
			} else {
				Err(SignError::IdentityUnavailable)
			}
		},
		|path, hardened, profile| developer.sign(path, hardened, profile),
		|path, executable, architectures, hardened, deep| {
			developer.verify(path, executable, architectures, hardened, deep)
		},
	)?;
	Ok(AppSigningReport {
		schema_version: REPORT_VERSION,
		backend: ResolvedBackend::Native,
		source: source.path().to_string_lossy().into_owned(),
		output: published.path.to_string_lossy().into_owned(),
		identity: developer.fingerprint().to_owned(),
		items: published.items,
		assessment: PolicyAssessment::NotPerformed,
	})
}

fn open_app(path: &Path) -> Result<Artifact, SignError> {
	let artifact = Artifact::open(path)?;
	if artifact.kind() == ArtifactKind::AppBundle {
		Ok(artifact)
	} else {
		Err(artifact.invalid(ArtifactProblem::Kind).into())
	}
}

struct Published {
	path: PathBuf,
	items: Vec<AppSigningItemReport>,
}

fn stage_and_sign(
	source: &Artifact,
	destination: &Path,
	rules: &[AppEntitlements<'_>],
	prepare_signer: impl FnOnce() -> Result<(), SignError>,
	mut sign_item: impl FnMut(&Path, bool, Option<&Path>) -> Result<(), SignError>,
	mut verify_item: impl FnMut(&Path, &Path, &[&str], bool, bool) -> Result<(), SignError>,
) -> Result<Published, SignError> {
	let stage = stage_result(Stage::new_app(destination, source.path()))?;
	stage_result(stage.validate())?;
	let original = copy::copy_bundle(source.path(), stage.path())
		.map_err(|source| SignError::AppCopy(Box::new(source)))?;
	source.validate()?;
	let staged = open_app(stage.path())?;
	stage_result(stage.validate_app(staged.file()))?;

	let items = plan::build(stage.path()).map_err(|source| SignError::AppPlan(Box::new(source)))?;
	let architectures = items
		.iter()
		.map(|item| architecture_names(&stage.path().join(&item.executable)))
		.collect::<Result<Vec<_>, _>>()?;
	let working = stage.path().parent().ok_or_else(|| {
		SignError::Stage(Box::new(std::io::Error::other(
			"app stage has no working directory",
		)))
	})?;
	let policies = entitlements::prepare(rules, &items, working)
		.map_err(|source| SignError::AppEntitlements(Box::new(source)))?;
	prepare_signer()?;
	for item in &items {
		stage_result(stage.validate_app(staged.file()))?;
		let path = stage.path().join(&item.relative);
		sign_item(&path, item.hardened, policies.path(&item.relative))?;
	}
	for (item, selected_architectures) in items.iter().zip(&architectures) {
		stage_result(stage.validate_app(staged.file()))?;
		let path = stage.path().join(&item.relative);
		let executable = stage.path().join(&item.executable);
		if architecture_names(&executable)? != *selected_architectures {
			return Err(SignError::AppArchitectureChanged);
		}
		verify_item(
			&path,
			&executable,
			selected_architectures,
			item.hardened,
			item.relative.as_os_str().is_empty(),
		)?;
		let reported_architectures = selected_architectures.iter().copied().map(Some);
		let default_architecture = selected_architectures.is_empty().then_some(None);
		for architecture in reported_architectures.chain(default_architecture) {
			policies
				.verify(&item.relative, &path, architecture)
				.map_err(|source| SignError::AppEntitlements(Box::new(source)))?;
		}
	}

	original
		.validate_source()
		.map_err(|source| SignError::AppCopy(Box::new(source)))?;
	source.validate()?;
	let items = items
		.iter()
		.map(|item| AppSigningItemReport {
			item: if item.relative.as_os_str().is_empty() {
				ROOT_ITEM.to_owned()
			} else {
				item.relative.to_string_lossy().into_owned()
			},
			hardened_runtime: item.hardened,
			entitlements: policies.path(&item.relative).is_some(),
		})
		.collect();
	drop(policies);
	let path = stage_result(stage.publish_app(staged.file()))?;
	Ok(Published { path, items })
}

fn stage_result<T>(result: Result<T, super::stage::Error>) -> Result<T, SignError> {
	result.map_err(|source| SignError::Stage(Box::new(source)))
}

fn architecture_names(executable: &Path) -> Result<Vec<&'static str>, SignError> {
	macho::architectures(executable).map_err(|source| SignError::AppPlan(Box::new(source)))
}
