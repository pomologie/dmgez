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
    "conversion": (["--no-default-features", "--features", "convert"], {"dmgez-core", "dmgez-operations", "dmgez-udif", "flate2"}),
    "parallel_conversion": (["--no-default-features", "--features", "convert,parallel"], {"dmgez-core", "dmgez-operations", "dmgez-udif", "flate2", "rayon"}),
    "comparison": (["--no-default-features", "--features", "diff"], {"dmgez-core", "dmgez-diff", "dmgez-udif", "dmgez-vfs", "flate2", "serde_json"}),
    "layout_lint": (["--no-default-features", "--features", "lint"], {"dmgez-config", "dmgez-lint", "dmgez-preview", "serde_json"}),
    "appdmg_import": (["--no-default-features", "--features", "appdmg-import"], {"dmgez-config", "dmgez-core", "serde_json"}),
}
OUTSIDE_DEFAULT = {
    "apple-dmg", "apfs", "apfs-container", "apfs-core", "apfs-format", "apfs-reader",
    "bzip2", "dmgez-apfs-read", "dmgez-hfsplus-read", "eframe", "egui", "fstool", "fuser",
    "hfsplus-reader", "lzfse_rust", "lzma-rust2", "notify",
    "reqwest", "rmcp", "tokio", "udif", "winfsp",
}
PROFILE_DEPS = {"dmgez-config", "dmgez-core", "dmgez-diff", "dmgez-inspector", "dmgez-lint", "dmgez-operations", "dmgez-preview", "dmgez-udif", "dmgez-vfs", "flate2", "rayon", "serde_json"}


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
            for feature in ("read", "write", "parallel", "zlib", "lzfse", "lzma"):
                active = f'dmgez-udif feature "{feature}"' in features
                expected_active = {
                    "read": profile in ("inspection", "conversion", "parallel_conversion", "comparison"),
                    "write": profile in ("serial", "default", "conversion", "parallel_conversion"),
                    "parallel": profile in ("default", "parallel_conversion"),
                    "zlib": True, "lzfse": False, "lzma": False,
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
