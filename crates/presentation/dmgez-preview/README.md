# Headless preview geometry

`ProjectFolder` combines a selected virtual folder's typed Finder settings with its resolved direct children. Root style supplies nested defaults and configured positions stay in their source folder. Exact configured centers survive zoom, viewport clipping, overlaps and drag calculations. Items without positions receive deterministic preview grid centers. The frontend can use each item's position source to distinguish fallback placement from a saved edit.

The preview geometry uses separate folder, canvas and Finder screen coordinates. The project adapter consumes already resolved virtual paths and carries configured background metadata. An actual-image adapter can feed decoded positions into the same geometry without routing them through a project. Asset decoding, text measurement, rendering, and native Finder comparison belong to their respective adapters.

The tests cover the coordinate and design-layout portion of D-GUI-01 and D-V11. The property test checks inverse viewport transforms across the documented test ranges and tolerance. Integration tests use synthetic project metadata, including an intentionally absent copy source and a virtual Applications link.

## Render a project design PNG

Build the CLI with `preview`. It can render the project root or a declared folder using the same layout geometry as the desktop.

```sh
cargo run --locked -p dmgez-cli --no-default-features --features preview -- \
  preview --project dmg.toml --output design.png --folder / --show-hidden
```

Omit `--show-hidden` to use the project’s normal visibility. The command prints `mode=design` and the output dimensions. It renders a manually arranged icon view with generic icons and text at one pixel per Finder point. It reads project declarations and the primary PNG background when configured. Contents copied into the final image do not need to exist to render their declared positions.

The PNG profile accepts the default background, solid colors, and static 8-bit RGB or RGBA backgrounds in sRGB or without a color profile. The primary PNG is drawn from the upper-left corner at native pixel size. Other background variants, custom icons, and automatic arrangement return errors. Rendering is capped at 512 entries in the selected folder, 4,096 pixels per side, and 4,194,304 pixels overall. The command creates a new output file.

Distributions of the CLI with `preview` include the embedded font notices. [Font notices](../../../apps/dmgez-desktop/THIRD_PARTY_NOTICES.md#embedded-fonts) · [CLI PNG tests](../../../apps/dmgez-cli/tests/preview_cli.rs)

## Render an actual disk image

Compose the CLI with the sibling HFS+ reader and run `preview` with an image path.

```sh
python3 tools/compose_filesystem_cli.py --hfsplus ../hfsplus --no-default-features --features actual-preview
target/hfsplus-cli/debug/dmgez preview installer.dmg --output actual.png
```

Use `--folder /Guide` for an exact stored folder path, `--partition SLOT` to select a partition, and `--raw` for a raw disk image. `--show-hidden` includes hidden entries. The command reads the real HFS+ or HFSX catalog and the selected folder’s `.DS_Store`. It uses supported icon-view settings, exact matches for stored icon positions, and generic icons. The report identifies the observed store and background and counts stored and fallback positions.

Solid colors and qualified PNG artwork are supported. For image aliases, the reader checks the in-volume stored path, catalog IDs, names, volume label, and creation dates before reading the asset. It reports fallback if the expected store or image cannot be used. The same pixel limits apply as in the design profile. [Actual-image tests](../../../apps/dmgez-cli/tests/preview_cli/actual.rs) · [Fixture and independent checks](../../../tests/fixtures/manifests/actual-preview-udzo.json)
