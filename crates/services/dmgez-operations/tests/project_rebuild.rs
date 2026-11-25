#![cfg(feature = "edit")]

use std::{collections::BTreeMap, fs, num::NonZeroUsize, path::Path};

use dmgez_builder::{
	planning::prepare,
	preflight::{Availability, BuildCapabilities, OutputPlan, ReleasePlan},
};
use dmgez_config::{
	VirtualPath,
	project_edit::{ContentChanges, ContentDraft, ProjectEdit},
};
use dmgez_operations::edit::{ProjectRebuildError, ProjectRebuildRequest, plan_project_rebuild};

const ORIGINAL_IMAGE: &str = "original.dmg";
const REBUILT_IMAGE: &str = "rebuilt.dmg";
const ORIGINAL_PROJECT: &str = "source.toml";
const INPUT_FILE: &str = "payload.txt";
const ORIGINAL_BYTES: &[u8] = b"existing image bytes";
const PAYLOAD: &[u8] = b"build payload";
const PROJECT: &str = r#"# Preserve this project header
schema_version = 1

[image]
name = "Original label"
output = "original.dmg"

[signing]
enabled = true

[[contents]]
source = "${INPUT}"
destination = "/Original.txt"
"#;

#[test]
fn plans_an_unsigned_rebuild_from_the_edited_project() {
	let directory = tempfile::tempdir().unwrap();
	let base = directory.path().canonicalize().unwrap();
	let project = base.join(ORIGINAL_PROJECT);
	let original = base.join(ORIGINAL_IMAGE);
	fs::write(&project, PROJECT).unwrap();
	fs::write(&original, ORIGINAL_BYTES).unwrap();
	fs::write(base.join(INPUT_FILE), PAYLOAD).unwrap();
	let current_source = fs::read_to_string(&project).unwrap();
	let mut changes = ContentChanges::default();
	changes.destination = Some("/Renamed.txt".parse().unwrap());
	let edits = [
		ProjectEdit::Modify {
			target: "/Original.txt".parse().unwrap(),
			changes,
		},
		ProjectEdit::Add(ContentDraft::Directory {
			destination: "/Docs".parse().unwrap(),
		}),
		ProjectEdit::SetOutputPath(REBUILT_IMAGE.to_owned()),
	];
	let mut request = request(&base, &edits);
	request.current_source = &current_source;
	let rebuild = plan_project_rebuild(request).unwrap();
	assert!(
		rebuild
			.source()
			.starts_with("# Preserve this project header\n")
	);
	assert!(rebuild.source().contains("${INPUT}"));
	assert_eq!(rebuild.output_path(), base.join(REBUILT_IMAGE));
	assert_eq!(
		rebuild.profile().release(),
		ReleasePlan::ConfiguredStagesDeferred
	);
	assert_eq!(
		rebuild.profile().output(),
		OutputPlan::Path { overwrite: false }
	);
	let prepared = prepare(
		rebuild.project(),
		rebuild.config_dir(),
		rebuild.options(),
		rebuild.profile().write_options().cancellation.clone(),
	)
	.unwrap();
	assert_eq!(prepared.output_path(), Some(rebuild.output_path()));
	let paths: Vec<_> = prepared
		.manifest()
		.entries()
		.iter()
		.map(|entry| entry.destination.as_str())
		.collect();
	assert_eq!(paths, ["/Docs", "/Renamed.txt"]);
	assert_eq!(
		prepared.manifest().regular_payload_bytes(),
		PAYLOAD.len() as u64
	);
	assert_eq!(fs::read_to_string(project).unwrap(), PROJECT);
	assert_eq!(fs::read(original).unwrap(), ORIGINAL_BYTES);
	assert!(!rebuild.output_path().exists());
}

#[test]
fn rejects_conflicting_edits_and_a_stale_revision_before_path_access() {
	let missing_base = Path::new("relative directory that the planner must not open");
	let mut stale = request(missing_base, &[]);
	stale.expected_source = "another revision";
	assert!(matches!(
		plan_project_rebuild(stale),
		Err(ProjectRebuildError::SourceChanged)
	));
	let target: VirtualPath = "/Original.txt".parse().unwrap();
	let edits = [
		ProjectEdit::Remove {
			target: target.clone(),
		},
		ProjectEdit::Remove { target },
	];
	assert!(matches!(
		plan_project_rebuild(request(missing_base, &edits)),
		Err(ProjectRebuildError::Project(_))
	));
}

#[test]
fn rejects_an_unchanged_destination_and_an_explicit_overwrite_request() {
	let directory = tempfile::tempdir().unwrap();
	let base = directory.path();
	let edits = [ProjectEdit::SetImageName("New label".to_owned())];
	assert!(matches!(
		plan_project_rebuild(request(base, &edits)),
		Err(ProjectRebuildError::UnchangedOutput)
	));
	let edits = [ProjectEdit::SetOutputPath(REBUILT_IMAGE.to_owned())];
	let original = format!("{PROJECT}\n[build]\noverwrite = true\n");
	let mut request = request(base, &edits);
	request.expected_source = &original;
	request.current_source = &original;
	assert!(matches!(
		plan_project_rebuild(request),
		Err(ProjectRebuildError::Overwrite)
	));
}

#[test]
fn rejects_an_existing_destination_without_touching_it() {
	let directory = tempfile::tempdir().unwrap();
	let base = directory.path();
	let output = base.join(REBUILT_IMAGE);
	fs::write(&output, ORIGINAL_BYTES).unwrap();
	let edits = [ProjectEdit::SetOutputPath(REBUILT_IMAGE.to_owned())];
	assert!(matches!(
		plan_project_rebuild(request(base, &edits)),
		Err(ProjectRebuildError::OutputExists { .. })
	));
	assert_eq!(fs::read(output).unwrap(), ORIGINAL_BYTES);
}

#[cfg(unix)]
#[test]
fn refuses_output_aliases_and_broken_symlinks() {
	use std::os::unix::fs::symlink;

	let directory = tempfile::tempdir().unwrap();
	let base = directory.path();
	symlink(base, base.join("alias")).unwrap();
	let edits = [ProjectEdit::SetOutputPath(format!(
		"alias/{ORIGINAL_IMAGE}"
	))];
	assert!(matches!(
		plan_project_rebuild(request(base, &edits)),
		Err(ProjectRebuildError::UnchangedOutput)
	));
	let output = base.join(REBUILT_IMAGE);
	symlink(base.join("missing"), &output).unwrap();
	let edits = [ProjectEdit::SetOutputPath(REBUILT_IMAGE.to_owned())];
	assert!(matches!(
		plan_project_rebuild(request(base, &edits)),
		Err(ProjectRebuildError::OutputExists { .. })
	));
	assert!(output.is_symlink());
}

fn request<'a>(base: &'a Path, edits: &'a [ProjectEdit]) -> ProjectRebuildRequest<'a> {
	let mut capabilities = BuildCapabilities::default();
	capabilities.hfsplus_unicode = Availability::Available;
	ProjectRebuildRequest {
		expected_source: PROJECT,
		current_source: PROJECT,
		config_dir: base,
		defines: BTreeMap::from([("INPUT".to_owned(), INPUT_FILE.to_owned())]),
		edits,
		capabilities,
		host_threads: NonZeroUsize::MIN,
	}
}
