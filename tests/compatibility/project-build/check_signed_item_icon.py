#!/usr/bin/env python3
"""Check that per-item icons preserve ad-hoc signed test inputs."""

import argparse
from hashlib import sha256
from pathlib import Path
import plistlib
import subprocess
import sys
from tempfile import TemporaryDirectory


BASE = Path(__file__).resolve().parent
ICON_FIXTURE = (
    BASE.parents[2] / "crates" / "presentation" / "dmgez-finder" /
    "tests" / "fixtures" / "apple-iconutil.icns"
)
CODESIGN = "/usr/bin/codesign"
XCRUN = "/usr/bin/xcrun"
BUNDLE = "Example.app"
EXTENSIONLESS_BUNDLE = "RenamedBundle"
FRAMEWORK = "Example.framework"
EXTENSIONLESS_FRAMEWORK = "RenamedFramework"
EXECUTABLE = "Example"
STANDALONE = "signed-executable"
SCRIPT = "signed-script"
PAYLOAD = "Contents/Resources/payload.txt"
FRAMEWORK_VERSION = "Versions/A"
FRAMEWORK_PAYLOAD = f"{FRAMEWORK_VERSION}/Resources/payload.txt"
ICON = "fixture.icns"
IMAGE = "existing.dmg"
SENTINEL = b"previous output from the test publisher\n"
APP_REFUSAL = "application bundle"
FRAMEWORK_REFUSAL = "or framework"
MACHO_REFUSAL = "executable signature"
XATTR_REFUSAL = "native code-signing metadata"
PROJECT = f"""\
schema_version = 1

[image]
name = "Signed icon refusal"
output = "{IMAGE}"
format = "udzo"
size = "1MiB"

[build]
overwrite = true
"""

OMIT_ATTRIBUTES = """\
[metadata]
preserve_finder_info = false
preserve_resource_forks = false
preserve_extended_attributes = false
"""

CONTENT = f"""\
[[contents]]
custom_icon = "{ICON}"
"""


def sign_fixture(bundle):
    payload = bundle / PAYLOAD
    payload.parent.mkdir(parents=True)
    payload.write_bytes(b"resource covered by the ad-hoc signature\n")
    executable = bundle / "Contents" / "MacOS" / EXECUTABLE
    executable.parent.mkdir()
    compile_executable(executable)
    (bundle / "Contents" / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleExecutable": EXECUTABLE,
        "CFBundleIdentifier": "com.pomologie.dmgez.fixture.item-icon",
        "CFBundleName": EXECUTABLE,
        "CFBundlePackageType": "APPL",
        "CFBundleVersion": "1",
    }))
    sign(bundle)


def compile_executable(output):
    subprocess.run(
        [XCRUN, "clang", "-x", "c", "-", "-o", output],
        input=b"int main(void) { return 0; }\n",
        check=True,
    )


def sign_framework(framework):
    version = framework / FRAMEWORK_VERSION
    payload = framework / FRAMEWORK_PAYLOAD
    payload.parent.mkdir(parents=True)
    payload.write_bytes(b"sealed framework resource\n")
    (payload.parent / "Info.plist").write_bytes(plistlib.dumps({
        "CFBundleExecutable": EXECUTABLE,
        "CFBundleIdentifier": "com.pomologie.dmgez.fixture.framework-icon",
        "CFBundleName": EXECUTABLE,
        "CFBundlePackageType": "FMWK",
        "CFBundleVersion": "1",
    }))
    subprocess.run([
        XCRUN, "clang", "-dynamiclib", "-install_name",
        f"@rpath/{FRAMEWORK}/{FRAMEWORK_VERSION}/{EXECUTABLE}",
        "-x", "c", "-", "-o", version / EXECUTABLE,
    ], input=b"int dmgez_fixture(void) { return 0; }\n", check=True)
    current = version.parent / "Current"
    current.symlink_to(version.name)
    (framework / EXECUTABLE).symlink_to(f"Versions/{current.name}/{EXECUTABLE}")
    (framework / "Resources").symlink_to(f"Versions/{current.name}/Resources")
    sign(framework)


def sign(path):
    subprocess.run([CODESIGN, "--force", "--sign", "-", "--timestamp=none", path], check=True)


def verify_signature(bundle):
    subprocess.run([CODESIGN, "--verify", "--strict", bundle], check=True)


def file_hashes(root):
    return {
        path.relative_to(root): sha256(path.read_bytes()).digest()
        for path in root.rglob("*") if path.is_file()
    }


def verify_refusal(runner, project, image, source, destination, diagnostic, metadata=""):
    project.write_text(
        PROJECT + metadata + CONTENT + f'source = "{source}"\ndestination = "{destination}"\n',
        encoding="utf-8",
    )
    result = subprocess.run(runner, capture_output=True, text=True)
    if result.returncode == 0 or diagnostic not in result.stderr.lower():
        raise RuntimeError(f"expected {diagnostic} refusal, got {result.stderr}")
    if image.read_bytes() != SENTINEL:
        raise RuntimeError("the refused operation changed existing output")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmgez", type=Path, required=True)
    parser.add_argument("--hfsplus", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("native signature verification requires macOS")

    with TemporaryDirectory(prefix="dmgez-signed-item-icon-") as temporary:
        root = Path(temporary).resolve()
        bundle = root / BUNDLE
        project = root / "project.toml"
        image = root / IMAGE
        image.write_bytes(SENTINEL)
        (root / ICON).write_bytes(ICON_FIXTURE.read_bytes())
        sign_fixture(bundle)
        verify_signature(bundle)
        original = file_hashes(bundle)
        runner = [
            sys.executable, BASE / "run.py", "--dmgez", args.dmgez, "--hfsplus", args.hfsplus,
            "--volume-icon", "--project", project,
        ]
        verify_refusal(runner, project, image, BUNDLE, "/Renamed", APP_REFUSAL)
        verify_refusal(runner, project, image, f"{BUNDLE}/{PAYLOAD}", "/payload.txt", APP_REFUSAL)
        verify_signature(bundle)
        if file_hashes(bundle) != original:
            raise RuntimeError("the signed application fixture changed")
        renamed = root / EXTENSIONLESS_BUNDLE
        bundle.rename(renamed)
        verify_signature(renamed)
        verify_refusal(
            runner, project, image, f"{EXTENSIONLESS_BUNDLE}/{PAYLOAD}", "/payload.txt", APP_REFUSAL,
        )
        verify_signature(renamed)
        if file_hashes(renamed) != original:
            raise RuntimeError("the renamed signed application fixture changed")

        framework = root / FRAMEWORK
        sign_framework(framework)
        verify_signature(framework)
        original = file_hashes(framework)
        renamed_framework = root / EXTENSIONLESS_FRAMEWORK
        framework.rename(renamed_framework)
        verify_signature(renamed_framework)
        verify_refusal(
            runner, project, image, EXTENSIONLESS_FRAMEWORK, "/Framework", FRAMEWORK_REFUSAL,
        )
        verify_refusal(
            runner, project, image, f"{EXTENSIONLESS_FRAMEWORK}/{FRAMEWORK_PAYLOAD}",
            "/payload.txt", FRAMEWORK_REFUSAL,
        )
        verify_signature(renamed_framework)
        if file_hashes(renamed_framework) != original:
            raise RuntimeError("the renamed signed framework fixture changed")

        standalone = root / STANDALONE
        script = root / SCRIPT
        compile_executable(standalone)
        script.write_bytes(b"#!/bin/sh\nexit 0\n")
        script.chmod(0o755)
        for path, diagnostic, metadata in (
            (standalone, MACHO_REFUSAL, ""),
            (script, XATTR_REFUSAL, OMIT_ATTRIBUTES),
        ):
            sign(path)
            verify_signature(path)
            contents = path.read_bytes()
            verify_refusal(runner, project, image, path.name, "/decorated", diagnostic, metadata)
            verify_signature(path)
            if path.read_bytes() != contents:
                raise RuntimeError("a signed file fixture changed")
    print("Icon requests on signed bundles and executables were refused. Signatures and output matched.")


if __name__ == "__main__":
    main()
