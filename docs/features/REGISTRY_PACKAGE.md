# dmgez

Create and inspect macOS disk images on macOS, Linux, and Windows. `dmgez` reads a TOML project, builds an HFS+ volume, and writes a DMG. The CLI supports portable ad hoc signing through `apple-codesign`.

## Install the CLI

```sh
cargo install dmgez --locked
dmgez --version
```

From the source package, use the included `examples/offline-cli` project. The manifest resolves input and output paths relative to its own directory.

```sh
dmgez build examples/offline-cli/dmg.toml --dry-run
dmgez build examples/offline-cli/dmg.toml --release
dmgez verify examples/offline-cli/example.dmg --container
dmgez sign verify examples/offline-cli/example.dmg --backend rust
```

`--release` applies the example's `[signing]` section and ad hoc signs the DMG. Omit `--release` for an unsigned build.

## Use the Rust library

```sh
cargo add dmgez
```

Use `dmgez::dmgbuilder` to plan or build unsigned images from a `dmg.toml`. Use `dmgez::reader` to open a DMG, read bytes in its decoded address space, and verify its declared container checksums.

```rust
use std::path::Path;

use dmgez::{dmgbuilder::{Mode, Project}, reader::Dmg};

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let manifest = Path::new("examples/offline-cli/dmg.toml");
	Project::open(manifest)?.run(Mode::Build)?;
	let mut image = Dmg::open(&manifest.with_file_name("example.dmg"))?;
	image.verify()?;
	let mut first_byte = [0];
	image.read_exact_at(0, &mut first_byte)?;
	Ok(())
}
```

Desktop releases are distributed separately.
