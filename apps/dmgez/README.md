# dmgez desktop

Launch the desktop app with `cargo run -p dmgez-gui` or pass a local TOML project or DMG path after `--`. The Open dialog and file parsing run through one bounded document worker. Use the platform Open and history shortcuts or navigate the controls with Tab.

The design workspace lays out project declarations and their implied folders. It uses the shared preview geometry for zoom, clipping, icon selection, and snapping. Volume-label, visibility, and position edits use the typed project document and preserve comments. Save copy creates a new TOML file beside the source project, keeping its relative paths valid.

Enable **Show layout findings** to check the current folder's declared icon layout. Visible icons with warnings get a marker on the canvas. The text list also includes findings for hidden and off-canvas icons. The check uses the same rules as `dmgez lint --profile layout` and refreshes after folder navigation, applied edits, undo, or redo. Dragging keeps the findings for the last applied edit until you release the icon. Dense layouts that exceed the desktop check's budget show an error.

Use **Add files…** to add up to 256 selected files to the current folder layout. Files below the project directory keep project-relative source paths. Files outside it use absolute source paths. A successful selection is one undoable edit.

The image workspace shows the inspector's observed UDIF metadata and partition discovery. Run details show logical and stored offsets in pages of 64. Use **Previous page**, **Next page**, or enter a page number in the page control. Paging uses the index from the opened file handle.

Choose **Verify container** to scan stored runs and declared container checksums in the background. The sidebar reports structure, payload, filesystem, and security coverage independently. Filesystem and security stay “Not checked” for a container scan. **Cancel check** stops at the next verification boundary and keeps the last completed result. If the open file changes, reopen the image before relying on its observed metadata.

The desktop tests cover bounded project input, native GPT fixture inspection, copy publication and no-clobber behavior, cancelled or failed opens, and grouped position edits with undo and redo. Tests for the run navigation portion of D-GUI-03 cover bounded pages, preservation of the open source, simulated accessibility activation, and cancellation. Verification tests use the native UDZO fixture and a copy with a changed payload byte.

A live smoke test on macOS 14.7.2 (23H311, arm64) opened the [native GPT fixture](../../crates/foundation/dmgez-vfs/tests/fixtures/native-gpt.dmg) through the standard file picker. The window reported GPT and a logical partition start of 20,480 bytes. Verification status stayed “Not checked”.

Run the desktop smoke suite from the workspace root.

```sh
cargo build --locked -p dmgez-gui
python3 apps/dmgez/tests/check_desktop.py --binary target/debug/dmgez-gui
```

Use the `.exe` suffix for a Windows executable. The optional binary check verifies the embedded font set without launching the app. See the [third-party notices](THIRD_PARTY_NOTICES.md) for desktop redistribution inputs.
