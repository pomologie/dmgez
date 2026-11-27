# dmgez

Create and inspect macOS disk images on macOS, Linux, and Windows. `dmgez` reads a TOML project, builds an HFS+ volume, and writes a DMG. The CLI supports portable ad hoc signing through `apple-codesign`.

## Install the CLI

```sh
cargo install dmgez --locked
dmgez --version
```

Copy `examples/offline-cli/dmg.toml` and `examples/offline-cli/hello.txt` from the source package or source checkout into an `examples/offline-cli` directory in your writable workspace. Run these commands from that workspace. Paths declared in the manifest resolve relative to the manifest directory.

```sh
dmgez build examples/offline-cli/dmg.toml --dry-run
dmgez build examples/offline-cli/dmg.toml --release --set image.output=examples/offline-cli/example-adhoc.dmg
dmgez verify examples/offline-cli/example-adhoc.dmg --container
dmgez sign verify examples/offline-cli/example-adhoc.dmg --backend rust
```

`--release` applies the example's `[signing]` section and ad hoc signs the DMG. The command-line output override is relative to the working directory. The Rust example below uses the manifest's default output, `example.dmg`.

## Use the Rust library

```sh
cargo new --bin dmgez-example
cd dmgez-example
cargo add dmgez
```

Copy both example files into `examples/offline-cli` inside `dmgez-example`. Put the Rust example below in `src/main.rs` and run `cargo run` from the project root.

Use `dmgez::dmgbuilder` to plan or build unsigned images from a `dmg.toml`. Use `dmgez::reader` to open a DMG, read bytes in its decoded address space, and verify its declared container checksums.

### Builder contract

- `Project::open(path)` reads and validates the manifest. Relative source and output paths resolve from the manifest directory.
- `project.run(Mode::Plan)` checks the inputs and seals the layout without writing an image. `Outcome::Planned` reports the intended destination and the logical disk size in bytes.
- `project.run(Mode::Build)` prepares the inputs again, stages the unsigned DMG, and publishes it to the manifest destination. The default is to refuse an existing destination. macOS and Linux support `build.overwrite = true` in the manifest. Windows requires a new destination. `Outcome::Built` reports the destination, logical disk size, and actual DMG size in bytes.

The plan is a report. A later build checks the current inputs and destination. The v0.1.0 release profile supports partitionless HFS+ images in UDRO and UDZO. The library exposes verification through `dmgez::reader` after the build. Use the CLI's `--release` workflow to apply `[signing]`.

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
