#!/usr/bin/env python3
"""Check source size and normal dependencies for each implemented CLI profile."""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MAX_LINES = 500
PROFILES = {
    "minimal": (["--no-default-features"], set()),
    "serial": (["--no-default-features", "--features", "build"], {"dmgez-core", "dmgez-udif", "flate2"}),
    "default": ([], {"dmgez-core", "dmgez-udif", "flate2", "rayon"}),
    "inspection": (["--no-default-features", "--features", "inspect"], {"dmgez-core", "dmgez-inspector", "dmgez-udif", "dmgez-vfs", "flate2", "serde_json"}),
}
OUTSIDE_DEFAULT = {
    "apple-dmg", "apfs", "bzip2", "eframe", "egui", "fstool", "fuser", "lzfse_rust", "notify",
    "reqwest", "rmcp", "tokio", "udif", "winfsp",
}
PROFILE_DEPS = {"dmgez-core", "dmgez-inspector", "dmgez-udif", "dmgez-vfs", "flate2", "rayon", "serde_json"}


def main():
    errors = []
    for directory in ("apps", "crates", "fuzz"):
        for path in sorted((ROOT / directory).rglob("*.rs")):
            if "target" not in path.relative_to(ROOT).parts:
                if len(path.read_text().splitlines()) > MAX_LINES:
                    errors.append(f"{path.relative_to(ROOT)} exceeds {MAX_LINES} lines")
    for profile, (flags, expected) in PROFILES.items():
        tree = subprocess.check_output(
            ["cargo", "tree", "--locked", "-p", "dmgez-cli", "-e", "normal",
             "--prefix", "none", "--format", "{p}", *flags], cwd=ROOT, text=True,
        )
        names = {line.split()[0] for line in tree.splitlines() if line.strip()}
        if names & PROFILE_DEPS != expected:
            errors.append(f"{profile} profile: expected {sorted(expected)}, found {sorted(names & PROFILE_DEPS)}")
        if "dmgez-udif" in expected:
            features = subprocess.check_output(
                ["cargo", "tree", "--locked", "-p", "dmgez-cli", "-e", "features",
                 "-i", "dmgez-udif", *flags], cwd=ROOT, text=True,
            )
            for feature in ("read", "write", "parallel", "zlib", "lzfse"):
                active = f'dmgez-udif feature "{feature}"' in features
                expected_active = {
                    "read": profile == "inspection", "write": profile in ("serial", "default"),
                    "parallel": profile == "default", "zlib": True, "lzfse": False,
                }[feature]
                if active != expected_active:
                    errors.append(f"{profile} profile: unexpected activation state for {feature}")
        if forbidden := names & OUTSIDE_DEFAULT:
            errors.append(f"{profile} profile: forbidden dependencies {sorted(forbidden)}")
    if errors:
        sys.exit("\n".join(errors))
    print("CLI dependency profiles and Rust source size passed")


if __name__ == "__main__":
    main()
