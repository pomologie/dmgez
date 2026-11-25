use std::{collections::BTreeSet, env, error::Error, ffi::OsStr, path::Path, process::Command};

mod profiles;
#[cfg(test)]
mod tests;

use profiles::{CHECKED_PACKAGES, FORBIDDEN_PACKAGES, PROFILES, Profile, Selection, UDIF_FEATURES};

const CARGO: &str = "cargo";
const CLI_PACKAGE: &str = "dmgez-cli";
const UDIF_PACKAGE: &str = "dmgez-udif";
const UDIF_FEATURE_PREFIX: &str = "dmgez-udif feature \"";
const NORMAL_EDGES: &str = "normal,no-proc-macro";
const FEATURE_EDGES: &str = "features";
const PACKAGE_FORMAT: &str = "{p}";

pub(crate) fn check(root: &Path) -> Result<(), Box<dyn Error>> {
	let cargo = env::var_os("CARGO").unwrap_or_else(|| CARGO.into());
	let mut errors = Vec::new();
	for profile in PROFILES {
		let packages = cargo_tree(root, &cargo, profile, Tree::Packages)?;
		let features = profile
			.packages
			.contains(&UDIF_PACKAGE)
			.then(|| cargo_tree(root, &cargo, profile, Tree::UdifFeatures))
			.transpose()?;
		check_profile(profile, &packages, features.as_deref(), &mut errors);
	}
	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors.join("\n").into())
	}
}

fn check_profile(
	profile: &Profile,
	packages: &str,
	features: Option<&str>,
	errors: &mut Vec<String>,
) {
	let expected: BTreeSet<_> = profile
		.packages
		.iter()
		.copied()
		.chain(profile.host_package)
		.collect();
	let names: BTreeSet<_> = packages
		.lines()
		.filter_map(|line| line.split_whitespace().next())
		.collect();
	let found = matching_packages(&names, CHECKED_PACKAGES);
	if found != expected {
		errors.push(format!(
			"{} profile: expected {}, found {}",
			profile.name,
			package_list(&expected),
			package_list(&found),
		));
	}
	if let Some(features) = features {
		let active: BTreeSet<_> = features
			.lines()
			.filter_map(|line| {
				let (feature, _) = line.strip_prefix(UDIF_FEATURE_PREFIX)?.split_once('"')?;
				Some(feature)
			})
			.collect();
		for feature in UDIF_FEATURES {
			if active.contains(feature) != profile.udif_features.contains(feature) {
				errors.push(format!(
					"{} profile: unexpected activation state for {feature}",
					profile.name,
				));
			}
		}
	}
	let forbidden = matching_packages(&names, FORBIDDEN_PACKAGES);
	if !forbidden.is_empty() {
		errors.push(format!(
			"{} profile: forbidden dependencies {}",
			profile.name,
			package_list(&forbidden),
		));
	}
}

fn matching_packages<'a>(names: &BTreeSet<&'a str>, policy: &[&str]) -> BTreeSet<&'a str> {
	names
		.iter()
		.copied()
		.filter(|name| policy.contains(name))
		.collect()
}

fn package_list(names: &BTreeSet<&str>) -> String {
	let names: Vec<_> = names.iter().map(|name| format!("'{name}'")).collect();
	format!("[{}]", names.join(", "))
}

#[derive(Clone, Copy)]
enum Tree {
	Packages,
	UdifFeatures,
}

impl Tree {
	const fn label(self) -> &'static str {
		match self {
			Self::Packages => NORMAL_EDGES,
			Self::UdifFeatures => FEATURE_EDGES,
		}
	}
}

fn cargo_tree(
	root: &Path,
	cargo: &OsStr,
	profile: &Profile,
	tree: Tree,
) -> Result<String, Box<dyn Error>> {
	let mut command = Command::new(cargo);
	command.current_dir(root).args([
		"tree",
		"--locked",
		"-p",
		CLI_PACKAGE,
		"-e",
		tree.label(),
		"--prefix",
		"none",
	]);
	match tree {
		Tree::Packages => command.args(["--format", PACKAGE_FORMAT]),
		Tree::UdifFeatures => command.args(["-i", UDIF_PACKAGE]),
	};
	match profile.selection {
		Selection::Default => {}
		Selection::Minimal => {
			command.arg("--no-default-features");
		}
		Selection::Only(features) => {
			command.args(["--no-default-features", "--features", features]);
		}
	}
	let output = command.output().map_err(|error| {
		format!(
			"{} profile: could not run cargo tree ({}): {error}",
			profile.name,
			tree.label()
		)
	})?;
	if !output.status.success() {
		return Err(format!(
			"{} profile: cargo tree ({}) failed with {}\n{}",
			profile.name,
			tree.label(),
			output.status,
			String::from_utf8_lossy(&output.stderr).trim_end(),
		)
		.into());
	}
	Ok(String::from_utf8(output.stdout).map_err(|error| {
		format!(
			"{} profile: cargo tree ({}) returned invalid UTF-8: {error}",
			profile.name,
			tree.label()
		)
	})?)
}
