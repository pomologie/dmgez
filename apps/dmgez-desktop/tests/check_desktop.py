#!/usr/bin/env python3
"""Check the desktop tests and the dependency profile reviewed for notices."""

import argparse
import json
import mmap
from pathlib import Path
import subprocess
import sys

REPO = Path(__file__).resolve().parents[3]
NOTICES = REPO / "apps/dmgez-desktop/licenses"
GUI = "dmgez-gui"
CLI = "dmgez-cli"
FONT = ("epaint_default_fonts", "0.37.0")
LOADER = ("libloading", "0.8.9")
FEATURE = "monochrome_emoji_fonts"
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
)
DESKTOP_DEPENDENCIES = {
    "accesskit", "dmgez-gui", "dmgez-preview", "eframe", "egui",
    "epaint", "glow", "glutin", "rfd", "wgpu",
}
NOTICE_FILES = (
    (FONT, "fonts/Hack-Regular.txt", "Hack-Regular.txt"),
    (FONT, "fonts/UFL.txt", "Ubuntu-Light-UFL.txt"),
    (FONT, "fonts/emoji-icon-font-mit-license.txt", "egui-icons-MIT.txt"),
    (LOADER, "LICENSE", "libloading-ISC.txt"),
)
FONT_BYTES = {
    "Hack-Regular.ttf": True,
    "Ubuntu-Light.ttf": True,
    "egui-icons.ttf": True,
    "NotoEmoji-Regular.ttf": False,
    "emoji-icon-font.ttf": False,
}
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def capture(*args):
    return subprocess.check_output(args, text=True, cwd=REPO)


def normal_dependencies(package, target):
    output = capture(
        "cargo", "tree", "--locked", "-p", package, "--target", target,
        "-e", "normal", "--prefix", "none", "--format", "{p}\t{f}",
    )
    for line in output.splitlines():
        package_id, _, features = line.removesuffix(" (*)").partition("\t")
        name, version, *_ = package_id.split()
        yield name, version.removeprefix("v"), set(features.split(","))


def checked_profile(target):
    dependencies = list(normal_dependencies(GUI, target))
    for package, expected in (FONT, LOADER):
        versions = {version for name, version, _ in dependencies if name == package}
        if versions != {expected}:
            raise ValueError(f"{target} desktop {package} versions {versions!r}")
    if any(
        name in {"epaint", FONT[0]} and FEATURE in features
        for name, _, features in dependencies
    ):
        raise ValueError(f"{target} desktop includes the optional monochrome fonts")
    if any(name == "wgpu" or name.startswith("wgpu-") for name, _, _ in dependencies):
        raise ValueError(f"{target} desktop includes the WGPU renderer")
    for name, _, _ in normal_dependencies(CLI, target):
        if any(
            name == dependency or name.startswith((dependency + "-", dependency + "_"))
            for dependency in DESKTOP_DEPENDENCIES
        ):
            raise ValueError(f"{target} default CLI includes desktop dependency {name}")


def checked_notice_packages():
    host = next(
        line.removeprefix("host: ")
        for line in capture("rustc", "-vV").splitlines() if line.startswith("host: ")
    )
    data = json.loads(capture(
        "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", host,
    ))
    packages = {
        (package["name"], package["version"]): Path(package["manifest_path"]).parent
        for package in data["packages"] if package["source"] == REGISTRY
    }
    for package, shipped, local in NOTICE_FILES:
        if (packages[package] / shipped).read_bytes() != (NOTICES / local).read_bytes():
            raise ValueError(f"desktop notice differs from the shipped text: {local}")
    return packages


def check_binary(binary, font_package):
    with binary.open("rb") as file, mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_READ) as image:
        for name, expected in FONT_BYTES.items():
            found = image.find((font_package / "fonts" / name).read_bytes()) != -1
            if found != expected:
                raise ValueError(f"unexpected embedded font state for {name}: {found}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, help="check font bytes in a built desktop executable")
    args = parser.parse_args()
    for target in TARGETS:
        checked_profile(target)
    packages = checked_notice_packages()
    if args.binary is not None:
        check_binary(args.binary, packages[FONT])
    subprocess.run(["cargo", "nextest", "run", "--locked", "-p", GUI], cwd=REPO, check=True)
    print("Desktop tests, dependency profiles, and shipped notices passed.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        sys.exit(f"Desktop smoke check failed: {error}")
