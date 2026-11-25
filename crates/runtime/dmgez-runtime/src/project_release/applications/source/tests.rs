use std::{
	fs,
	num::NonZeroUsize,
	os::unix::fs::symlink,
	path::{Path, PathBuf},
};

use dmgez_builder::{
	planning,
	preflight::{Availability, BuildCapabilities, OutputIntent, PreflightOptions, ReleaseIntent},
};
use dmgez_config::Project;

use super::*;

const PROJECT: &str = r#"
schema_version = 1
[image]
name = "Release fixture"
output = "output.dmg"
format = "udro"
"#;
const BUNDLE_COPY: &str = r#"
[[contents]]
source = "Source.app"
destination = "/Dock/PRODUCT.APP"
"#;
const SOURCE_APP: &str = "Source.app";
const RELEASE_APP: &str = "/Dock/PRODUCT.APP";
const SEALED_MEMBER: &str = "Contents/Resource";
#[cfg(feature = "finder-write")]
const STORE_NAME: &str = ".DS_Store";

struct Fixture {
	_directory: tempfile::TempDir,
	root: PathBuf,
}

impl Fixture {
	fn new() -> Self {
		let directory = tempfile::tempdir().unwrap();
		let root = directory.path().canonicalize().unwrap();
		let app = root.join(SOURCE_APP);
		fs::create_dir_all(app.join("Contents/Helper.app/Contents")).unwrap();
		fs::write(app.join(SEALED_MEMBER), b"source resource").unwrap();
		fs::write(
			app.join("Contents/Helper.app/Contents/Nested"),
			b"nested resource",
		)
		.unwrap();
		symlink("Resource", app.join("Contents/Alias")).unwrap();
		Self {
			_directory: directory,
			root,
		}
	}

	fn source(&self) -> PathBuf {
		self.root.join(SOURCE_APP)
	}

	fn plan(&self, contents: &str) -> PreparedBuild {
		let project = Project::parse(&format!("{PROJECT}{contents}")).unwrap();
		let mut capabilities = BuildCapabilities::default();
		capabilities.hfsplus_ascii = Availability::Available;
		capabilities.hfsplus_unicode = Availability::Available;
		capabilities.finder_flags = Availability::Available;
		capabilities.finder_layout = Availability::Available;
		planning::prepare(
			&project,
			&self.root,
			PreflightOptions {
				capabilities,
				output: OutputIntent::Path,
				release: ReleaseIntent::Unsigned,
				host_threads: NonZeroUsize::MIN,
			},
			Default::default(),
		)
		.unwrap()
	}
}

#[test]
fn binds_exact_outermost_bundle_with_nested_code_and_an_adjacent_name() {
	let fixture = Fixture::new();
	let plan = fixture.plan(&format!(
		"{BUNDLE_COPY}\n[[contents]]\ntype = 'directory'\ndestination = '{RELEASE_APP}-archive'"
	));
	let mut verified = Vec::new();
	let apps = prepare(&plan, |path| {
		verified.push(path.to_path_buf());
		assert_eq!(
			fs::read_link(path.join("Contents/Alias")).unwrap(),
			Path::new("Resource")
		);
		Ok(())
	})
	.unwrap();
	assert_eq!(verified, [fixture.source()]);
	assert_eq!(
		apps.iter().map(VirtualPath::as_str).collect::<Vec<_>>(),
		[RELEASE_APP]
	);
}

#[test]
fn refuses_an_omitted_member_and_a_rebound_source_with_the_same_virtual_name() {
	let fixture = Fixture::new();
	let filtered = format!("{BUNDLE_COPY}\nexclude = ['{SEALED_MEMBER}']\n");
	let plan = fixture.plan(&filtered);
	assert!(
		prepare(&plan, |_| panic!(
			"verification must not run for a filtered bundle"
		))
		.unwrap_err()
		.to_string()
		.contains(INCOMPLETE_APP)
	);
	fs::write(fixture.root.join("Replacement"), b"distinct source").unwrap();
	let plan = fixture.plan(&format!(
		"{filtered}\n[[contents]]\nsource = 'Replacement'\ndestination = '{RELEASE_APP}/{SEALED_MEMBER}'"
	));
	assert!(
		prepare(&plan, |_| panic!(
			"verification must not run for a rebound member"
		))
		.unwrap_err()
		.to_string()
		.contains(INCOMPLETE_APP)
	);
}

#[test]
fn refuses_added_members_and_overrides_before_verification() {
	let fixture = Fixture::new();
	let added = fixture.plan(&format!(
		"{BUNDLE_COPY}\n[[contents]]\ntype = 'directory'\ndestination = '{RELEASE_APP}/Generated'"
	));
	assert!(
		prepare(&added, |_| panic!(
			"verification must not run for an added member"
		))
		.unwrap_err()
		.to_string()
		.contains(INCOMPLETE_APP)
	);
	let hidden = fixture.plan(&format!("{BUNDLE_COPY}\nhidden = true"));
	assert!(
		prepare(&hidden, |_| panic!(
			"verification must not run for changed metadata"
		))
		.unwrap_err()
		.to_string()
		.contains(MODIFIED_APP)
	);
}

#[cfg(feature = "finder-write")]
#[test]
fn allows_a_parent_icon_position_but_refuses_a_generated_store_inside_the_bundle() {
	let fixture = Fixture::new();
	let parent_position = fixture.plan(&format!(
		"{BUNDLE_COPY}\nposition = [100, 100]\n[finder]\nwindow_size = [400, 300]"
	));
	assert!(prepare(&parent_position, |_| Ok(())).is_ok());
	let internal_store = fixture.plan(&format!("{BUNDLE_COPY}\n[finder]\nwindow_size = [400, 300]\n[[finder.folders]]\npath = '{RELEASE_APP}/Contents'\nshow_toolbar = true"));
	assert!(
		internal_store
			.finder_assets()
			.iter()
			.any(|asset| asset.path.as_str() == format!("{RELEASE_APP}/Contents/{STORE_NAME}"))
	);
	assert!(
		prepare(&internal_store, |_| panic!(
			"verification must not run for a generated bundle member"
		))
		.unwrap_err()
		.to_string()
		.contains(GENERATED_IN_APP)
	);
}

#[test]
fn rejects_source_mutation_during_verification_even_when_the_verifier_itself_fails() {
	let fixture = Fixture::new();
	let plan = fixture.plan(BUNDLE_COPY);
	let error = prepare(&plan, |path| {
		fs::write(
			path.join(SEALED_MEMBER),
			b"resource changed during verification",
		)?;
		Err(io::Error::other("negative verifier result").into())
	})
	.unwrap_err();
	assert!(
		matches!(error.downcast_ref::<dmgez_builder::manifest::Error>(),
			Some(dmgez_builder::manifest::Error::SourceChanged { path }) if path == &fixture.source().join(SEALED_MEMBER)
		)
	);
}
