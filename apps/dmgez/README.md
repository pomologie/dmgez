# dmgez desktop

Launch the desktop app with `cargo run -p dmgez-gui` or pass a local TOML project or DMG path after `--`. The Open dialog and file parsing run through one bounded document worker. Use the platform Open and history shortcuts or navigate the controls with Tab.

**Recent files** lists up to 12 opened local projects or images across launches. **Clear recent files** removes those paths. Turn off **Remember recent files** to clear the list and stop recording.

The design workspace lays out project declarations and their implied folders. It uses the shared preview geometry for zoom, clipping, icon selection, and snapping. Volume-label, visibility, and position edits use the typed project document and preserve comments. Save copy creates a new TOML file beside the source project, keeping its relative paths valid.

On macOS and Linux, **Save** and Cmd/Ctrl+S save the exact edited TOML back to the open project. Save compares the file identity and original text before atomic replacement in a trusted local directory. Conflicts and unsupported access metadata leave your edits available in the editor. **Save copy** is available on Windows as well.

Enable **Show layout findings** to check the current folder's declared icon layout. Visible icons with warnings get a marker on the canvas. The text list also includes findings for hidden and off-canvas icons. The check uses the same rules as `dmgez lint --profile layout` and refreshes after folder navigation, applied edits, undo, or redo. Dragging keeps the findings for the last applied edit until you release the icon. Dense layouts that exceed the desktop check's budget show an error.

Use **Add files…** to add up to 256 selected files to the current folder layout. Files below the project directory keep project-relative source paths. Files outside it use absolute source paths. A successful selection is one undoable edit.

### Build a project from the desktop

Enable the HFS+ builder when composing the desktop app.

```sh
python3 tools/compose_filesystem_cli.py --hfsplus ../hfsplus --desktop --features project-build
target/hfsplus-desktop/debug/dmgez-gui dmg.toml
```

**Plan build** checks the current project and shows the planned image geometry. **Build image…** asks where to publish a fresh unsigned image. Both use a snapshot of the editor, including unsaved changes, with source paths relative to the open project's directory. The build panel shows progress, cancellation and metadata omissions. Open a published UDIF image from its result to inspect it in the desktop.

### Inspect a disk image

The image workspace shows the inspector's observed UDIF metadata and partition discovery. Run details show logical and stored offsets in pages of 64. Use **Previous page**, **Next page**, or enter a page number in the page control. Paging uses the index from the opened file handle.

Enter a decimal or `0x` hexadecimal offset in **Logical byte offset** and choose **Go to run** or press Enter. The inspector opens the page containing that byte and highlights its run using the retained index from the opened image.

Choose a codec or decoder status under **Filter runs**, then choose **Sort runs** and select **Apply run query**. Results cover the whole admitted run index and keep their original run numbers across pages. Select a row to export that run. **Clear run query** restores logical order, and **Go to run** opens the logical page containing the requested byte.

Choose **Verify container** to scan stored runs and declared container checksums in the background. The sidebar reports structure, payload, filesystem, and security coverage independently. Filesystem and security stay “Not checked” for a container scan. **Cancel** stops at the next verification boundary and keeps the last completed result. If the open file changes, reopen the image before relying on its observed metadata.

Export the current inspection or completed verification as JSON from the sidebar. **Export selected run…** saves the selected run's decoded logical bytes when the decoder is available and the run is at most 64 MiB. Exports require a fresh destination and recheck the open image before publication.

Use **Compare…** to compare the open image with another DMG. Choose logical disk bytes, stored image bytes, or a filesystem supported by the composed desktop build. A logical difference can take you to its run. Filesystem results show the changed paths and their fields. Use **Disk and metadata selection** to choose partition slots or APFS object IDs and checkpoint XIDs when an image has more than one candidate.

### Browse HFS+ and HFSX files

From the workspace root, compose the desktop app with the sibling HFS+ reader.

```sh
python3 tools/compose_filesystem_cli.py --hfsplus ../hfsplus --desktop
target/hfsplus-desktop/debug/dmgez-gui installer.dmg
```

Choose **Files** to open a recognized whole-disk HFS+ or HFSX volume or select a partition data window to check. Folder listings show up to 64 entries per page and preserve the stored names. Use **Up**, **Root**, or the breadcrumbs to navigate. **Details** shows the entry type, owner, group, permissions, and available catalog and link information. Symlink targets are displayed as stored data. Folder reads run in the background on the opened DMG handle and can be cancelled.

Regular file details report logical data and stored resource fork lengths separately. Extended attribute details show up to 64 stored names. The view distinguishes an empty inventory from an unavailable observation and reports the cause of a failed query.

Choose **Preview text** in a regular file’s details to read up to 64 KiB of its logical data fork. The preview shows selectable UTF-8 text and reports whether it reached the end of the file. Binary content and unsupported encodings receive separate messages.

Choose **Preview PNG** to display one still 8-bit PNG directly from the selected file. The decoder reads up to 4 MiB of encoded data and permits up to 2,048 pixels along either side and 1,048,576 pixels in total. The preview uses the decoded color samples directly.

Use **Back** and **Forward** to revisit folders and their last catalog pages. **Find in folder** searches up to 4,096 immediate catalog entries and shows up to 64 matches in native catalog order. It uses the volume's name matching rules and indicates when a limit stops the search.

Add `--features lzfse,lzma` to enable ULFO and ULMO decoding for verification and filesystem reads. Container metadata remains visible when its decoder is unavailable.

Add `--features actual-preview` to the desktop composer and choose **Preview folder** after opening a volume or folder in **Files**. The preview shows actual stored icon positions and supported artwork, with fallback status next to the image. Use **Show catalog list** to return to folder navigation and file details. [Actual-image preview profile](../../crates/presentation/dmgez-preview/README.md#render-an-actual-disk-image)

Activate a rendered folder icon to browse it, or activate a file, application bundle, or symlink icon to open its details. The icon controls are keyboard accessible. **Show hidden items** redraws the preview from the opened volume, and the setting carries across folder navigation.

Add `--test` to the composer command to run the desktop suite with the sibling reader’s fixtures. The macOS suite exercised an Apple partition map, exact decomposed HFS+ names, separate case-sensitive HFSX entries, pagination, cancellation, and a modified source. An AccessKit simulation opened the Files view and navigated the paged catalog. [Volume tests](src/image/browse/tests.rs)

On arm64 macOS 14.7.2 (23H311), the composed desktop opened a UDRO image of the sibling reader’s synthetic catalog fixture through the file picker. The live Files view showed vertically aligned rows on pages 1–64 and 65–97. Opening `Nested` showed the stored decomposed name `Résumé.txt` with its mode, owner and catalog ID in **Details**. The fixture’s SHA-256 matched before and after browsing.

### Browse APFS files

Compose the desktop with the sibling APFS reader.

```sh
python3 tools/compose_filesystem_cli.py --apfs ../apfs --desktop
target/apfs-desktop/debug/dmgez-gui archive.dmg
```

In **Files**, check the whole disk or select a partition to check for an APFS container. Choose a volume from the displayed checkpoint. Folder navigation, details, and file previews retain that checkpoint and volume identity throughout the opened image session. Search matches the exact stored spelling and case in the current folder. Add `--hfsplus ../hfsplus` to compose both readers in `target/hfsplus-apfs-desktop`.

Add `--test` for native APFS fixture tests. Finder layout rendering is available for HFS+ and HFSX.

### Desktop smoke checks

The desktop tests cover bounded project input, native GPT fixture inspection, copy publication and no-clobber behavior, cancelled or failed opens, and grouped position edits with undo and redo. Tests for the run navigation portion of D-GUI-03 cover bounded pages, preservation of the open source, simulated accessibility activation, and cancellation. Verification tests use the native UDZO fixture and a copy with a changed payload byte.

A live smoke test on macOS 14.7.2 (23H311, arm64) opened the [native GPT fixture](../../crates/foundation/dmgez-vfs/tests/fixtures/native-gpt.dmg) through the standard file picker. The window reported GPT and a logical partition start of 20,480 bytes. Verification status stayed “Not checked”.

Run the desktop smoke suite from the workspace root.

```sh
cargo build --locked -p dmgez-gui
python3 apps/dmgez/tests/check_desktop.py --binary target/debug/dmgez-gui
```

Use the `.exe` suffix for a Windows executable. The optional binary check verifies the embedded font set without launching the app. See the [third-party notices](THIRD_PARTY_NOTICES.md) for desktop redistribution inputs.
