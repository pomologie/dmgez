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
```

The CLI stages outputs in the destination directory and refuses to replace existing files. `unwrap-raw --partition` takes a one-based slot reported by `inspect --partitions` and exports that slot's complete logical extent. The default writer uses bounded parallel compression. A serial-only CLI can be built with `--no-default-features --features build`.

The [project-build compatibility runner](tests/compatibility/project-build/README.md) builds version-one TOML projects using the HFS+ builder from a sibling checkout or unpacked packages. File-tree builds in the default CLI require a resolvable versioned HFS+ dependency.

The [native desktop app](apps/dmgez/README.md) opens TOML projects for layout editing and disk images for container and partition inspection. Launch it with `cargo run --locked -p dmgez-gui`.

See the [architecture](ARCHITECTURE.md), [roadmap](ROADMAP.md), and [UDIF evidence](docs/reference/UDIF_EVIDENCE.md) for the implemented profile and its verification limits.
