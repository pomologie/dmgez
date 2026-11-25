#!/usr/bin/env bash
set -euo pipefail

verification_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --harness checked_range_matches_mathematical_window
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features read \
	--harness indexed_read_cursor
cargo kani --manifest-path "$verification_dir/../Cargo.toml" \
	--no-default-features --features read \
	--harness run_admission --output-format terse
