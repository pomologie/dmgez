#!/usr/bin/env python3
"""Exercise dmgez's unsigned project builder across isolated processes (D-PERF-02)."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory


PROJECT = "dmg.toml"
IMAGE = "out.dmg"
CONTENTS = "contents"
BUILD_EPOCH = 946684800
CHUNK_BYTES = 16 * 1024
VOLUME_BYTES = 1024 * 1024
REPEATED_CHUNKS = 12
SERIAL = 0
PARALLEL_WORKERS = (1, 2, 4, 8)
PROCESS_TIMEOUT = 30
PAYLOAD = "Codes/payload.bin"
DESTINATION = "/Files"

MANIFEST = f"""schema_version = 1

[image]
name = "Deterministic fixture"
output = "{IMAGE}"
format = "udzo"
size = {VOLUME_BYTES}
shrink = false

[build]
reproducible = true
source_date_epoch = {BUILD_EPOCH}

[metadata]
preserve_timestamps = false
preserve_ownership = false
preserve_finder_info = false
preserve_extended_attributes = false
preserve_resource_forks = false

[compression]
block_size = {CHUNK_BYTES}
level = 6

[[contents]]
source = "{CONTENTS}"
destination = "{DESTINATION}"
position = [140, 100]

[[contents]]
type = "symlink"
destination = "/Applications"
target = "/Applications"
position = [350, 100]

[finder]
window_size = [520, 280]
background = {{ kind = "color", value = "#f0f0f0" }}
"""


def run(directory, *command, environment=None):
	try:
		return subprocess.run(
			command,
			cwd=directory,
			env=environment,
			check=True,
			capture_output=True,
			timeout=PROCESS_TIMEOUT,
		).stdout
	except subprocess.CalledProcessError as error:
		output = (error.stdout + error.stderr).decode("utf-8", errors="replace").strip()
		raise ValueError(f"{directory.name}: {command[1]} exited {error.returncode}\n{output}") from error


def sha256(path):
	with path.open("rb") as data:
		return hashlib.file_digest(data, "sha256").hexdigest()


def materialize(directory, reverse):
	directory.mkdir()
	tree = directory / CONTENTS
	tree.mkdir(mode=0o755)
	payload = (
		hashlib.shake_256(b"dmgez reproducibility witness").digest(CHUNK_BYTES * REPEATED_CHUNKS)
		+ b"z" * (CHUNK_BYTES * REPEATED_CHUNKS)
	)
	files = {
		"Docs/ReadMe.txt": b"Reproducible fixture\n",
		"Docs/Équipement.txt": b"Stable catalog name and bytes\n",
		PAYLOAD: payload,
		".hidden": b"Included by the project\n",
	}
	order = reversed(files) if reverse else files
	for name in order:
		path = tree / name
		path.parent.mkdir(mode=0o755, exist_ok=True)
		path.write_bytes(files[name])
		path.chmod(0o644)
	for path in (tree, tree / "Docs", tree / "Codes"):
		path.chmod(0o755)
	(directory / PROJECT).write_text(MANIFEST, encoding="utf-8")


def build(binary, directory, workers, *overrides):
	parallel = "true" if workers else "false"
	environment = os.environ.copy()
	# The manifest explicitly controls generated timestamps in every process.
	environment["SOURCE_DATE_EPOCH"] = str(BUILD_EPOCH + workers + 1)
	run(
		directory,
		binary,
		"build",
		PROJECT,
		"--set",
		f"performance.parallel={parallel}",
		"--set",
		f"performance.threads={workers}",
		*overrides,
		environment=environment,
	)
	return directory / IMAGE


def verify_layout(binary, image):
	report = json.loads(run(image.parent, binary, "inspect", "--json", image))
	container = report["container"]
	counts = container["runs"]
	# The profile must exercise multiple batches, with both compressed and
	# incompressible chunks surviving in the output map.
	if (
		container["logical_bytes"] != str(VOLUME_BYTES)
		or counts["zlib"] <= max(PARALLEL_WORKERS)
		or not counts["raw"]
	):
		raise ValueError(f"fixture must exercise mixed UDZO runs across multiple batches: {counts}")
	run(image.parent, binary, "verify", image)
	expected = (image.parent / CONTENTS / PAYLOAD).read_bytes()
	decoded = run(image.parent, binary, "cat", image, f"{DESTINATION}/{PAYLOAD}", "--fork", "data")
	if decoded != expected:
		raise ValueError("HFS+ data fork differs from the source payload")


def verify_native(binary, baseline):
	directory = baseline.parent
	captured = directory / "decoded.raw"
	native = directory / "native"
	run(directory, binary, "unwrap-raw", baseline, "-o", captured)
	run(directory, "/usr/bin/hdiutil", "verify", "-quiet", baseline)
	run(directory, "/usr/bin/hdiutil", "convert", "-quiet", baseline, "-format", "UDTO", "-o", native)
	native_digest = sha256(native.with_suffix(".cdr"))
	if sha256(captured) != native_digest:
		raise ValueError("native UDTO export differs from dmgez's decoded disk")
	print(f"Apple-verified decoded SHA-256 {native_digest}")


def verify_matrix(binary, native):
	with TemporaryDirectory(prefix="dmgez-reproducibility.") as temporary:
		root = Path(temporary)
		initial = root / "0-serial"
		materialize(initial, reverse=False)
		baseline = build(binary, initial, SERIAL)
		baseline_digest = sha256(baseline)
		verify_layout(binary, baseline)
		print(f"{initial.name} {baseline_digest}")
		for index, workers in enumerate((*PARALLEL_WORKERS, SERIAL), start=1):
			label = f"workers-{workers}" if workers else "serial"
			directory = root / f"{index}-{label}"
			materialize(directory, reverse=bool(index % 2))
			image = build(binary, directory, workers)
			digest = sha256(image)
			run(
				root, binary, "diff", "--mode", "semantic", "--filesystem", "hfsplus",
				"--finder-info", baseline, image,
			)
			if digest != baseline_digest:
				raise ValueError(f"{directory.name}: SHA-256 {digest} differs from {baseline_digest}")
			print(f"{directory.name} {digest}")
		changed = root / "changed-epoch"
		materialize(changed, reverse=False)
		changed_image = build(
			binary, changed, SERIAL, "--set", f"build.source_date_epoch={BUILD_EPOCH + 1}",
		)
		changed_digest = sha256(changed_image)
		if changed_digest == baseline_digest:
			raise ValueError("a different explicit build epoch left the generated image unchanged")
		print(f"{changed.name} {changed_digest}")
		if native:
			verify_native(binary, baseline)


def main():
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument("--dmgez", type=Path, required=True, help="composed dmgez CLI binary")
	parser.add_argument("--native", action="store_true", help="compare Apple UDTO export on macOS")
	options = parser.parse_args()
	if options.native and sys.platform != "darwin":
		parser.error("--native requires macOS and hdiutil")
	try:
		verify_matrix(options.dmgez.resolve(strict=True), options.native)
	except (OSError, KeyError, ValueError, subprocess.TimeoutExpired) as error:
		print(error, file=sys.stderr)
		return 1
	return 0


if __name__ == "__main__":
	sys.exit(main())
