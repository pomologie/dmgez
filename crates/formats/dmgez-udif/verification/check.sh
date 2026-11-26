#!/usr/bin/env bash
set -euo pipefail

verification_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --harness checked_range_matches_mathematical_window
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features read --harness binary_geometry_proof --output-format terse
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features write --harness write_geometry_proof
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features write --harness writer_cursor_proof --output-format terse
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features read \
	--harness indexed_read_
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features read \
	--harness run_admission --output-format terse
