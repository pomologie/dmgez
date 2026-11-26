"""Generate a review draft from the exact composed CLI Cargo workspace."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory


CARGO_ABOUT_VERSION = "cargo-about 0.9.3"
NOTICE_DRAFT_HEADER = "dmgez dependency notices draft"
CONFIG = Path(__file__).with_name("release-notices.toml").resolve()
CLI_MANIFEST = Path("apps/dmgez-cli/Cargo.toml")
LOCKFILE = "Cargo.lock"


def host_target(cargo, workspace):
    return subprocess.check_output(
        [*cargo[:-1], "rustc", "--print", "host-tuple"], cwd=workspace, encoding="utf-8",
    ).strip()


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def package_label(package):
    return f'{package["name"]} {package["version"]}'


def render(data, target, binary_hash, lock_hash, features, no_defaults):
    packages = {
        package_label(entry["package"]): entry["license"] for entry in data["crates"]
    }
    if len(packages) != len(data["crates"]):
        raise ValueError("license report contains duplicate package labels")
    sections = []
    covered = set()
    fallback = set()
    for license_info in data["licenses"]:
        identifier = license_info["id"]
        text = license_info["text"]
        users = tuple(sorted(package_label(item["crate"]) for item in license_info["used_by"]))
        if not users or not text.strip():
            raise ValueError("license report contains an empty license section")
        canonical = not license_info["source_path"]
        if canonical:
            fallback.update((identifier, user) for user in users)
        covered.update(users)
        sections.append((identifier, users, canonical, text))
    if not covered or covered != packages.keys():
        raise ValueError("license sections do not cover the reported Cargo packages")

    lines = [
        NOTICE_DRAFT_HEADER,
        "Review required before release.",
        f"Target = {target}",
        f"CLI SHA-256 = {binary_hash}",
        f"Composed Cargo.lock SHA-256 = {lock_hash}",
        f"Default features = {'disabled' if no_defaults else 'enabled'}",
        f"Additional features = {' '.join(features)}",
        f"License tool = {CARGO_ABOUT_VERSION}",
        "",
        "Resolved packages and selected licenses",
        *(f"{label} | {license_id}" for label, license_id in sorted(packages.items())),
        "",
        "Canonical license texts requiring source notice review",
        *(f"{label} | {identifier}" for identifier, label in sorted(fallback)),
        "",
        "License texts",
    ]
    for identifier, users, canonical, text in sorted(sections):
        origin = "Canonical SPDX text" if canonical else "Source license text"
        lines.extend(("", f"{identifier} | {origin}", ", ".join(users), "", text))
    return "\n".join(lines) + "\n"


def check_built_packages(report, built_packages):
    reported = {entry["package"]["id"] for entry in report["crates"]}
    if reported != built_packages:
        missing = ", ".join(sorted(built_packages - reported)) or "none"
        extra = ", ".join(sorted(reported - built_packages)) or "none"
        raise ValueError(f"license report disagrees with the build packages (missing {missing}, extra {extra})")


def generate(cargo, workspace, binary, target, features, no_defaults, output, *, built_packages):
    version = subprocess.check_output(
        [*cargo, "about", "--version"], cwd=workspace, encoding="utf-8",
    ).strip()
    if version != CARGO_ABOUT_VERSION:
        raise ValueError(f"notices generation requires {CARGO_ABOUT_VERSION}, found {version}")
    command = [
        *cargo, "about", "generate", "--frozen", "--fail", "--format", "json",
        "--target", target, "--manifest-path", CLI_MANIFEST, "--config", CONFIG,
    ]
    if no_defaults:
        command.append("--no-default-features")
    for group in features:
        command.extend(("--features", group))
    report = json.loads(subprocess.check_output(command, cwd=workspace, encoding="utf-8"))
    check_built_packages(report, built_packages)
    text = render(report, target, digest(binary), digest(workspace / LOCKFILE), features, no_defaults)
    with TemporaryDirectory(prefix=".dmgez-notices.", dir=output.parent) as temporary:
        stage = Path(temporary) / "notices"
        stage.write_bytes(text.encode("utf-8"))
        os.link(stage, output)
