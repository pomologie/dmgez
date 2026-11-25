# Offline CLI archives

Prepare an archive from a trusted native [composed CLI](../../README.md#build-from-a-toml-project) built from this checkout. Enable `inspect` when composing it so the packaged CLI can verify the image built from the example. Supply a reviewed UTF-8 dependency notice file for the actual build target and capabilities. Python 3.11 or newer is required to prepare the archive. An extracted CLI runs on its target host without Python, Cargo or the filesystem source checkouts.

Run from the checkout on the same operating system as the supplied executable. The output directory must already exist on a filesystem that supports hard links. Use a fresh path.

```sh
python3 -B tools/package_cli.py \
  --binary target/hfsplus-cli/release/dmgez \
  --target aarch64-apple-darwin \
  --notices /path/to/THIRD_PARTY_NOTICES.txt \
  --output /path/to/dmgez-0.1.0-aarch64-apple-darwin.tar.gz
```

Use `.tar.gz` for macOS and Linux. Use `.zip` with `dmgez.exe` on Windows. The supported targets are `aarch64` and `x86_64` on macOS Darwin, Linux GNU and Windows MSVC. Use the target triple the CLI was built with. The packager checks the executable format, machine, version, and required capabilities. It prints the finished archive's SHA-256 and filename. The `dmgez-<version>-<target>` folder contains the executable, project license, dependency notices, two-file example and `SHA256SUMS`. The macOS and Linux archives include `install.sh`. Archives with the portable signer also include the covered vendored source, its license and provenance. File modes, member order and timestamps are fixed for repeatable packaging of the same input bytes.

Extract the archive into a writable directory and verify `SHA256SUMS` from inside the extracted folder with `shasum -a 256 -c SHA256SUMS` on macOS or `sha256sum -c SHA256SUMS` on Linux. Build and verify the included project from that folder.

```sh
./dmgez build example/dmg.toml
./dmgez verify example/example.dmg --container
```

On macOS and Linux, run `./install.sh` from the extracted folder to install `dmgez` into `~/.local/bin`. Pass `--bin-dir DIRECTORY` to choose another destination. Pass `--replace` to upgrade an existing different copy. The installer verifies the executable against `SHA256SUMS` before copying it. Ensure your chosen directory is on `PATH` to run `dmgez` by name.

On Windows, use `Expand-Archive` on a local NTFS volume. Use `Get-FileHash -Algorithm SHA256 .\dmgez.exe` and compare the value with its `SHA256SUMS` entry. Then run the example.

```powershell
.\dmgez.exe build example\dmg.toml
.\dmgez.exe verify example\example.dmg --container
```

The [offline packaging test](../../tools/test_package_cli.py) accepts the same `--binary`, `--target` and `--notices` inputs. It compares two archives byte for byte, verifies every extracted file against the checksum ledger, and builds and verifies the example with the extracted executable. On macOS and Linux it also installs the extracted binary into a disposable directory and runs the installed copy.
