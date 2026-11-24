# dmgez

A pure Rust library and CLI for creating, reading, and manipulating macOS disk images.

The CLI wraps an existing sector-aligned raw image in single-segment UDRO or UDZO. The optional `inspect` feature enables metadata inspection, container checksum verification, and raw export. The `dmgez-config` crate parses and validates version-one project manifests before the build layer accesses source paths. File-tree builds through the CLI still require integration with the sibling HFS+ builder.

```sh
cargo run --locked -- wrap-raw volume.raw -o volume.dmg
cargo run --locked -- wrap-raw volume.raw --format udro --serial -o uncompressed.dmg
cargo run --locked --features inspect -- inspect volume.dmg
cargo run --locked --features inspect -- verify volume.dmg
cargo run --locked --features inspect -- unwrap-raw volume.dmg -o exported.raw
```

The CLI stages outputs in the destination directory and refuses to replace existing files. The default writer uses bounded parallel compression. A serial-only CLI can be built with `--no-default-features --features build`.

See the [architecture](ARCHITECTURE.md), [roadmap](ROADMAP.md), and [UDIF evidence](docs/reference/UDIF_EVIDENCE.md) for the implemented profile and its verification limits.
