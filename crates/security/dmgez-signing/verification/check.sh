#!/usr/bin/env bash
set -euo pipefail

verification_dir=$(cd -- "$(dirname -- "$0")" && pwd)
jar=${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the pinned TLA+ Tools v1.8.0 JAR}
case "$jar" in
	/*) ;;
	*) jar="$PWD/$jar" ;;
esac

readonly expected_sha256=fdffef247f563ef6edbfd44958d2a82db79e7564e07cb673262d896353fa6713
readonly violation_exit=12
if command -v sha256sum >/dev/null; then
	checksum=$(sha256sum < "$jar")
else
	checksum=$(LC_ALL=C shasum -a 256 < "$jar")
fi
if [[ ${checksum%% *} != "$expected_sha256" ]]; then
	printf '%s\n' 'TLA2TOOLS_JAR does not match the v1.8.0 SHA-256 pin' >&2
	exit 1
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/dmgez-release-tlc.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
cd -- "$verification_dir/models"

run_tlc() {
	local scenario=$1
	java -Xmx256m -XX:+UseParallelGC -jar "$jar" \
		-workers 1 -seed 0 -fp 0 -noGenerateSpecTE \
		-metadir "$scratch/$scenario" -config "$scenario.cfg" StagedRelease.tla
}

expect_violation() {
	local scenario=$1 invariant=$2 status=0
	local log=$scratch/$scenario.log
	run_tlc "$scenario" > "$log" 2>&1 || status=$?
	if [[ $status != "$violation_exit" ]] ||
		! grep -Fxq -- "Error: Invariant $invariant is violated." "$log"; then
		cat -- "$log" >&2
		printf 'Expected %s to violate %s (TLC exit %d)\n' "$scenario" "$invariant" "$status" >&2
		exit 1
	fi
	cat -- "$log"
}

run_tlc StagedRelease
expect_violation StaleTicket StaplingAuthorized
expect_violation BlindRetry SingleSubmission
