#!/usr/bin/env bash
set -euo pipefail

model_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
jar=${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the official v1.8.0 tla2tools.jar}
case "$jar" in
	/*) ;;
	*) jar="$PWD/$jar" ;;
esac

readonly expected_sha256=fdffef247f563ef6edbfd44958d2a82db79e7564e07cb673262d896353fa6713
readonly invariant_violation_status=12
actual_sha256=$(LC_ALL=C shasum -a 256 < "$jar")
if [[ ${actual_sha256%% *} != "$expected_sha256" ]]; then
	printf 'TLC JAR checksum mismatch at %s\n' "$jar" >&2
	exit 1
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/dmgez-udif-tlc.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
cd -- "$model_dir"

run_tlc() {
	local scenario=$1
	java -XX:+UseParallelGC -Xmx512m -jar "$jar" \
		-workers 1 -fp 0 -seed 0 -noGenerateSpecTE \
		-metadir "$scratch/$scenario" -config "$scenario.cfg" OrderedBatch.tla
}

expect_violation() {
	local scenario=$1 invariant=$2 status=0
	local log="$scratch/$scenario.log"
	run_tlc "$scenario" > "$log" 2>&1 || status=$?
	cat -- "$log"
	if [[ $status -ne $invariant_violation_status ]] ||
		! grep -Fxq -- "Error: Invariant $invariant is violated." "$log"; then
		printf 'Expected %s to violate %s (TLC exit %d)\n' "$scenario" "$invariant" "$status" >&2
		exit 1
	fi
}

run_tlc ordered-batch
expect_violation completion-order OrderedConsumption
expect_violation lost-credit CreditsReleasedOnTermination
