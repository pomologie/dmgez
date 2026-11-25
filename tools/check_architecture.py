#!/usr/bin/env python3
"""Check source size and normal dependencies for each implemented CLI profile."""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MAX_LINES = 500
RUST_SIGNING_DEPS = {"dmgez-core", "dmgez-signing", "dmgez-udif", "flate2", "serde_json", "sha2"}
if sys.platform in ("darwin", "linux", "win32"):
    RUST_SIGNING_DEPS.add("apple-codesign")
NATIVE_SIGNING_DEPS = {"dmgez-core", "dmgez-signing", "dmgez-udif", "serde_json", "sha2"}
if sys.platform == "darwin":
    NATIVE_SIGNING_DEPS.add("dmgez-macho")
PROFILES = {
    "minimal": (["--no-default-features"], set()),
    "serial": (["--no-default-features", "--features", "build"], {"dmgez-core", "dmgez-udif", "flate2"}),
    "default": ([], RUST_SIGNING_DEPS | {"rayon"}),
    "inspection": (["--no-default-features", "--features", "inspect"], {"dmgez-core", "dmgez-inspector", "dmgez-udif", "dmgez-vfs", "flate2", "serde_json"}),
    "remote_inspection": (["--no-default-features", "--features", "remote"],
                          {"dmgez-core", "dmgez-inspector", "dmgez-udif", "dmgez-vfs", "flate2", "rustls", "serde_json", "ureq"}),
    "conversion": (["--no-default-features", "--features", "convert"], {"dmgez-core", "dmgez-operations", "dmgez-udif", "flate2"}),
    "parallel_conversion": (["--no-default-features", "--features", "convert,parallel"], {"dmgez-core", "dmgez-operations", "dmgez-udif", "flate2", "rayon"}),
    "comparison": (["--no-default-features", "--features", "diff"], {"dmgez-core", "dmgez-diff", "dmgez-udif", "dmgez-vfs", "flate2", "serde_json"}),
    "hash_comparison": (["--no-default-features", "--features", "diff-hash"], {"blake3", "dmgez-core", "dmgez-diff", "dmgez-udif", "dmgez-vfs", "flate2", "serde_json"}),
    "finder_comparison": (["--no-default-features", "--features", "finder-diff"], {"dmgez-config", "dmgez-core", "dmgez-diff", "dmgez-finder", "dmgez-udif", "dmgez-vfs", "ds_parser", "flate2", "serde_json"}),
    "layout_lint": (["--no-default-features", "--features", "lint"], {"dmgez-config", "dmgez-lint", "dmgez-preview", "serde_json"}),
    "design_preview": (["--no-default-features", "--features", "preview"], {"dmgez-config", "dmgez-core", "dmgez-preview", "epaint", "flate2", "png"}),
    "installer_lint": (["--no-default-features", "--features", "installer-lint"], {"dmgez-builder", "dmgez-config", "dmgez-core", "dmgez-lint", "dmgez-preview", "dmgez-udif", "serde_json"}),
    "appdmg_import": (["--no-default-features", "--features", "appdmg-import"], {"dmgez-config", "dmgez-core", "serde_json"}),
    "rust_signing": (["--no-default-features", "--features", "signing-rust"], RUST_SIGNING_DEPS),
    "native_signing": (["--no-default-features", "--features", "signing-native"], NATIVE_SIGNING_DEPS),
    "native_notarization": (["--no-default-features", "--features", "notarization-native"], NATIVE_SIGNING_DEPS),
}
OUTSIDE_DEFAULT = {
    "apple-dmg", "apfs", "apfs-container", "apfs-core", "apfs-format", "apfs-reader",
    "bzip2", "dmgez-apfs-read", "dmgez-cache", "dmgez-hfsplus-read", "dmgez-mount", "eframe", "egui", "fstool", "fuser",
    "hfsplus-reader", "lzfse_rust", "lzma-rust2", "notify",
    "reqwest", "rmcp", "tokio", "udif", "winfsp",
}
PROFILE_DEPS = {"apple-codesign", "blake3", "dmgez-builder", "dmgez-config", "dmgez-core", "dmgez-diff", "dmgez-finder", "dmgez-inspector", "dmgez-lint", "dmgez-macho", "dmgez-operations", "dmgez-preview", "dmgez-signing", "dmgez-udif", "dmgez-vfs", "ds_parser", "epaint", "flate2", "png", "rayon", "rustls", "serde_json", "sha2", "ureq"}


def main():
    errors = []
    for directory in ("apps", "crates", "fuzz"):
        for path in sorted((ROOT / directory).rglob("*.rs")):
            if "target" not in path.relative_to(ROOT).parts:
                if len(path.read_text().splitlines()) > MAX_LINES:
                    errors.append(f"{path.relative_to(ROOT)} exceeds {MAX_LINES} lines")
    for profile, (flags, expected) in PROFILES.items():
        tree = subprocess.check_output(
            ["cargo", "tree", "--locked", "-p", "dmgez-cli", "-e", "normal,no-proc-macro",
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
                    "read": profile in ("default", "rust_signing", "inspection", "remote_inspection", "conversion", "parallel_conversion", "comparison", "hash_comparison", "finder_comparison"),
                    "write": profile in ("serial", "default", "conversion", "parallel_conversion", "installer_lint"),
                    "parallel": profile in ("default", "parallel_conversion"),
                    "zlib": profile not in ("installer_lint", "native_signing", "native_notarization"), "lzfse": False, "lzma": False,
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
