# Offline CLI archives

Prepare an archive from a trusted native composed CLI built from this checkout. Keep the default Rust signer and enable `inspect` so the packaged CLI can build, sign, and verify the bundled project. Supply a reviewed UTF-8 dependency notice file for the actual target and capabilities. Python 3.11 or newer is required to prepare the archive. An extracted CLI runs on its target host without Python, Cargo or the filesystem source checkouts. [CLI composition](../../README.md#build-from-a-toml-project)

To prepare a target-specific draft, install `cargo-about` 0.9.3 and compose the optimized CLI with the notice output option. The license tool scans the same locked temporary workspace used to build the binary. The composer checks its report against the packages Cargo built. The draft records the binary and lockfile hashes, native target, feature selection, and resolved licenses. [License tool](https://github.com/EmbarkStudios/cargo-about)

```sh
python3 -B tools/compose_hfsplus_cli.py --hfsplus ../hfsplus \
  --release --features inspect --notices-output /path/to/THIRD_PARTY_NOTICES.draft.txt \
  --notices-lock-output /path/to/composed-Cargo.lock
```

Use the executable path printed by the composer. Review the draft against the dependency sources, including any canonical fallback text and source notices. The draft includes supplemental source files from the locked packages where the scanner omits component notices or publisher attributions. These include ring's Fiat Crypto attribution and the Apache notice for shlex. In the reviewed file, change the heading to `dmgez dependency notices v1`, remove the review-required line, and retain the recorded build identity. The packager checks the target and both SHA-256 fields against the staged executable and the exact signer sources written into the archive. Regenerate older drafts that lack the source digest. The packager also accepts manually prepared notices.

The macOS arm64 build at `57eecd7` and its notice tool reported the same 133 Cargo package IDs. The review verified 117 registry archive checksums and compared the retained license texts and supplemental notices, including 37 canonical fallback entries, with the previously reviewed macOS arm64 archive.

The locked `binrw` and `binrw_derive` 0.15.2 crate archives declare MIT but omit the license file. The draft includes the exact upstream license from the shared commit recorded in their Cargo VCS metadata. The retained file has SHA-256 `08159509fe99d146416fe110053a7360ddfccbba5f0c8f955bdbda025d4d2a20`. [Upstream license at the packaged revision](https://raw.githubusercontent.com/jam1garner/binrw/db9ef7bece9525057001edd260439fa2748287df/LICENSE)

For the Linux CLI, the draft also retains the locked `linux-raw-sys` 0.12.1 crate's separate `COPYRIGHT` file alongside its selected Apache license texts.

For the Windows CLI, the draft retains the packaged source copyright headers for six locked MIT crates whose archives omit standalone license text. Offline scans of both Windows MSVC targets confirmed their source notices and the architecture-specific Microsoft import-library license. These scans cover source attribution. The notice for a shipped Windows executable still comes from its own composed build.

Run from the checkout on the same operating system as the supplied executable. The output directory must already exist on a filesystem that supports hard links. Use a fresh path.

```sh
python3 -B tools/package_cli.py \
  --binary /path/to/the/composed/dmgez \
  --target aarch64-apple-darwin \
  --notices /path/to/THIRD_PARTY_NOTICES.txt \
  --output /path/to/dmgez-0.1.0-aarch64-apple-darwin.tar.gz
```

Use `.tar.gz` for macOS and Linux. Use `.zip` with `dmgez.exe` on Windows. The supported targets are `aarch64` and `x86_64` on macOS Darwin, Linux GNU and Windows MSVC. Use the target triple the CLI was built with. The packager checks the executable format, machine, version, and required capabilities. It prints the finished archive's SHA-256 and filename. The `dmgez-<version>-<target>` folder contains the executable, project license, dependency notices, two-file example and `SHA256SUMS`. The macOS and Linux archives include `install.sh`. Archives with the portable signer also include the covered vendored source, its license and provenance. File modes, member order and timestamps are fixed for repeatable packaging of the same input bytes.

Generate a CycloneDX 1.6 SBOM sidecar from the archive and the composed lockfile retained with the notice draft.

```sh
python3 -B tools/release_sbom.py \
  --archive /path/to/dmgez-0.1.0-aarch64-apple-darwin.tar.gz \
  --target aarch64-apple-darwin \
  --lockfile /path/to/composed-Cargo.lock \
  --output /path/to/dmgez-0.1.0-aarch64-apple-darwin.cdx.json
```

The SBOM records the Cargo build inventory, including build-time packages, and binds it to the archive, executable, reviewed notices and composed lockfile by digest. Registry package hashes identify the crate archives. Publish the sidecar and its checksum alongside the CLI archive. [CycloneDX 1.6 JSON reference](https://cyclonedx.org/docs/1.6/json/)

Extract the archive into a writable directory and verify `SHA256SUMS` from inside the extracted folder with `shasum -a 256 -c SHA256SUMS` on macOS or `sha256sum -c SHA256SUMS` on Linux. Build and verify the included project from that folder.

```sh
./dmgez build example/dmg.toml
./dmgez verify example/example.dmg --container
./dmgez build example/dmg.toml --release --set image.output=example/example-adhoc.dmg
./dmgez sign verify example/example-adhoc.dmg --backend rust
```

On macOS and Linux, run `./install.sh` from the extracted folder to install `dmgez` into `~/.local/bin`. Pass `--bin-dir DIRECTORY` to choose another destination. Pass `--replace` to upgrade an existing different copy. The installer verifies the executable against `SHA256SUMS` before copying it. Ensure your chosen directory is on `PATH` to run `dmgez` by name.

On Windows, use `Expand-Archive` on a local NTFS volume. Use `Get-FileHash -Algorithm SHA256 .\dmgez.exe` and compare the value with its `SHA256SUMS` entry. Then run the example.

```powershell
.\dmgez.exe build example\dmg.toml
.\dmgez.exe verify example\example.dmg --container
.\dmgez.exe build example\dmg.toml --release --set image.output=example/example-adhoc.dmg
.\dmgez.exe sign verify example\example-adhoc.dmg --backend rust
```

The offline packaging test accepts the same `--binary`, `--target` and `--notices` inputs. It compares two archives byte for byte, verifies every extracted file against the checksum ledger, and uses the extracted executable to build, ad hoc sign, and verify the example. On macOS and Linux it also installs the extracted binary into a disposable directory and runs the installed copy. [Packaging test](../../tools/test_package_cli.py)

The project-build workflow runs this test on Linux, macOS and Windows with the executable built by that runner. It supplies an explicitly labeled fixture notice and discards the test archives. Each job uploads the exact tested executable, its generated notice draft, the composed lockfile and the source revisions. The Windows artifact is named `windows-msvc-notice-review`. The Linux and macOS artifacts use `notice-review-<target>`. Review the draft for the target before packaging the shipped executable. [Project qualification workflow](../../.github/workflows/project-build.yml)
