use std::{env, error::Error, fs, path::PathBuf};

use dmgez::{
	dmgbuilder::{Mode, Outcome, Project},
	reader::Dmg,
};

fn main() -> Result<(), Box<dyn Error>> {
	let manifest = PathBuf::from(env::args_os().nth(1).ok_or("expected a manifest path")?);
	let project_directory = manifest.canonicalize()?.parent().unwrap().to_path_buf();
	let project = Project::open(&manifest)?;
	let Outcome::Planned {
		output: Some(path),
		logical_bytes,
		..
	} = project.run(Mode::Plan)?
	else {
		return Err("expected a file-backed plan".into());
	};
	assert_eq!(path.parent(), Some(project_directory.as_path()));
	assert!(!path.exists());
	let Outcome::Built {
		output: Some(output),
		physical_bytes,
		..
	} = project.run(Mode::Build)?
	else {
		return Err("expected a published image".into());
	};
	assert_eq!(output, path);
	let published = fs::read(&output)?;
	assert!(project.run(Mode::Build).is_err());
	assert_eq!(fs::read(&output)?, published);
	let mut image = Dmg::open(&output)?;
	assert_eq!(image.logical_bytes(), logical_bytes);
	assert_eq!(image.physical_bytes(), physical_bytes);
	let mut byte = [0];
	image.read_exact_at(0, &mut byte)?;
	assert!(image.read_exact_at(logical_bytes, &mut byte).is_err());
	let verification = image.verify()?;
	assert!(verification.data_fork && verification.master);
	assert!(!verification.tables.is_empty() && verification.tables.iter().all(|&checked| checked));
	Ok(())
}
