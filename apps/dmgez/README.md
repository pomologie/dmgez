# dmgez desktop

Launch the desktop app with `cargo run -p dmgez-gui` or pass a local TOML project or DMG path after `--`. The Open dialog and file parsing run through one bounded document worker. Use the platform Open and history shortcuts or navigate the controls with Tab.

The design workspace lays out project declarations and their implied folders. It uses the shared preview geometry for zoom, clipping, icon selection, and snapping. Volume-label, visibility, and position edits use the typed project document and preserve comments. Save copy creates a new TOML file beside the source project, keeping its relative paths valid.

Enable **Show layout findings** to check the current folder's declared icon layout. Visible icons with warnings get a marker on the canvas. The text list also includes findings for hidden and off-canvas icons. The check uses the same rules as `dmgez lint --profile layout` and refreshes after folder navigation, applied edits, undo, or redo. Dragging keeps the findings for the last applied edit until you release the icon. Dense layouts that exceed the desktop check's budget show an error.

Use **Add files…** to add up to 256 selected files to the current folder layout. Files below the project directory keep project-relative source paths. Files outside it use absolute source paths. A successful selection is one undoable edit.

The image workspace shows the inspector's observed UDIF metadata and partition discovery. Run details show logical and stored offsets in pages of 64. Use **Previous page**, **Next page**, or enter a page number in the page control. Paging uses the index from the opened file handle.

Choose **Verify container** to scan stored runs and declared container checksums in the background. The sidebar reports structure, payload, filesystem, and security coverage independently. Filesystem and security stay “Not checked” for a container scan. **Cancel** stops at the next verification boundary and keeps the last completed result. If the open file changes, reopen the image before relying on its observed metadata.

### Browse HFS+ and HFSX files

From the workspace root, compose the desktop app with the sibling HFS+ reader.

```sh
python3 tools/compose_filesystem_cli.py --hfsplus ../hfsplus --desktop
target/hfsplus-desktop/debug/dmgez-gui installer.dmg
```

Choose **Files** to open a recognized whole-disk HFS+ or HFSX volume or select a partition data window to check. Folder listings show up to 64 entries per page and preserve the stored names. Use **Up**, **Root**, or the breadcrumbs to navigate. **Details** shows the entry type, owner, group, permissions, and available catalog and link information. Symlink targets are displayed as stored data. Folder reads run in the background on the opened DMG handle and can be cancelled.

Add `--features lzfse,lzma` to enable ULFO and ULMO decoding for verification and filesystem reads. Container metadata remains visible when its decoder is unavailable.

Add `--features actual-preview` to the desktop composer and choose **Preview folder** after opening a volume or folder in **Files**. The preview shows actual stored icon positions and supported artwork, with fallback status next to the image. Use **Show catalog list** to return to folder navigation and file details. [Actual-image preview profile](../../crates/presentation/dmgez-preview/README.md#render-an-actual-disk-image)

Add `--test` to the composer command to run the desktop suite with the sibling reader’s fixtures. The macOS suite exercised an Apple partition map, exact decomposed HFS+ names, separate case-sensitive HFSX entries, pagination, cancellation, and a modified source. An AccessKit simulation opened the Files view and navigated the paged catalog. [Volume tests](src/image/browse/tests.rs)

On arm64 macOS 14.7.2 (23H311), the composed desktop opened a UDRO image of the sibling reader’s synthetic catalog fixture through the file picker. The live Files view showed vertically aligned rows on pages 1–64 and 65–97. Opening `Nested` showed the stored decomposed name `Résumé.txt` with its mode, owner and catalog ID in **Details**. The fixture’s SHA-256 matched before and after browsing.

### Desktop smoke checks

The desktop tests cover bounded project input, native GPT fixture inspection, copy publication and no-clobber behavior, cancelled or failed opens, and grouped position edits with undo and redo. Tests for the run navigation portion of D-GUI-03 cover bounded pages, preservation of the open source, simulated accessibility activation, and cancellation. Verification tests use the native UDZO fixture and a copy with a changed payload byte.

A live smoke test on macOS 14.7.2 (23H311, arm64) opened the [native GPT fixture](../../crates/foundation/dmgez-vfs/tests/fixtures/native-gpt.dmg) through the standard file picker. The window reported GPT and a logical partition start of 20,480 bytes. Verification status stayed “Not checked”.

Run the desktop smoke suite from the workspace root.

```sh
cargo build --locked -p dmgez-gui
python3 apps/dmgez/tests/check_desktop.py --binary target/debug/dmgez-gui
```

Use the `.exe` suffix for a Windows executable. The optional binary check verifies the embedded font set without launching the app. See the [third-party notices](THIRD_PARTY_NOTICES.md) for desktop redistribution inputs.
