# dmgez

A pure Rust library and CLI for creating, reading, and manipulating macOS disk images.

The CLI wraps an existing sector-aligned raw image in single-segment UDRO or UDZO. Enable `lzfse` for ULFO or `lzma` for ULMO. The optional `inspect` feature enables metadata reports, run-space analysis, container verification, and raw export.

```sh
cargo run --locked -- wrap-raw volume.raw -o volume.dmg
cargo run --locked -- wrap-raw volume.raw --format udro --serial -o uncompressed.dmg
cargo run --locked --features lzfse -- wrap-raw volume.raw --format ulfo -o compressed.dmg
cargo run --locked --features lzma -- wrap-raw volume.raw --format ulmo -o compressed.dmg
cargo run --locked --features inspect -- inspect volume.dmg
cargo run --locked --features inspect -- inspect volume.dmg --json --run-limit 32
cargo run --locked --features inspect -- inspect volume.dmg --partitions --json
cargo run --locked --features inspect -- analyze volume.dmg --json
cargo run --locked --features inspect -- verify volume.dmg
cargo run --locked --features inspect -- verify volume.dmg --quick --json
cargo run --locked --features inspect -- unwrap-raw volume.dmg -o exported.raw
cargo run --locked --features inspect -- unwrap-raw volume.dmg --partition 2 -o partition.raw
cargo run --locked --no-default-features --features convert -- convert volume.dmg --format udro -o uncompressed.dmg
cargo run --locked --no-default-features --features convert -- optimize volume.dmg -o optimized.dmg
cargo run --locked --no-default-features --features diff -- diff volume.dmg uncompressed.dmg --json
cargo run --locked --no-default-features --features lint -- lint project.toml --profile layout --folder /Docs --json
cargo run --locked --no-default-features --features appdmg-import -- import-appdmg settings.json -o project.toml --image-output installer.dmg
```

The CLI stages outputs in the destination directory and refuses to replace existing files. `unwrap-raw --partition` takes a one-based slot reported by `inspect --partitions` and exports that slot's complete logical extent. The default writer uses bounded parallel compression. A serial-only CLI can be built with `--no-default-features --features build`.

The optional `convert` command rebuilds qualified unsigned UDRO and UDZO containers. It verifies the source and compares the staged output's decoded data and admitted metadata before publication. It refuses inputs carrying opaque resources, signatures, or unqualified block maps.

The same feature enables `optimize`. It samples aligned windows within `--max-sample-chunks` and `--max-sample-bytes`, chooses the smaller measured UDRO or UDZO sample result, and publishes a verified conversion. It reports sampled offsets, wrapped sample sizes, and observed encoder times. Those measurements apply to the sampled windows.

The optional `diff` command compares decoded logical disk bytes by default. Use `--mode physical` to compare stored files or `--max-bytes` to change the comparison budget. Exit codes are 0 for equality, 1 for a difference, and 2 for an error or an exhausted budget. Reports identify the address space and length of the equal prefix. Filesystem semantics and integrity verification are separate checks.

The optional `lint` command checks configured Finder icon geometry in one declared project folder, starting at `/` unless `--folder` selects another. It reads the TOML manifest only. Use `--suppress RULE=REASON` to keep a finding in the report without failing on it. Exit codes are 0 when there are no unsuppressed warnings, 1 when warnings remain, and 2 when the requested layout cannot be evaluated.

`import-appdmg` translates documented appdmg JSON settings into a new version-one TOML manifest. Relative asset paths keep their original settings directory. A relative `--image-output` is interpreted from the new manifest's directory. The command reports unmapped behavior and refuses to replace an existing manifest.

## Build from a TOML project

While the versioned HFS+ dependencies are unpublished, compose the standalone CLI locally on macOS or Linux using a sibling checkout or unpacked 0.1.0 packages. The compose tool uses a temporary Cargo workspace and the local Cargo cache. The resulting binary contains the builder and runs directly.

```sh
python3 tools/compose_hfsplus_cli.py --hfsplus ../hfsplus
target/hfsplus-cli/debug/dmgez build project.toml --dry-run
target/hfsplus-cli/debug/dmgez build project.toml
```

Use `--hfsplus-packages PATH` for unpacked packages, or `--no-default-features --features build` for a serial build with Finder support. The compose tool's `--test` mode runs the build command's integration tests. Use `dmgez capabilities` to check whether the binary includes project building. The [compatibility runner](tests/compatibility/project-build/README.md) exercises the same build service and command implementation with the native fixtures.

The [native desktop app](apps/dmgez/README.md) opens TOML projects for layout editing and disk images for container and partition inspection. Launch it with `cargo run --locked -p dmgez-gui`.

See the [architecture](ARCHITECTURE.md), [roadmap](ROADMAP.md), and [UDIF evidence](docs/reference/UDIF_EVIDENCE.md) for the implemented profile and its verification limits.
