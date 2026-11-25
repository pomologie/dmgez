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

scratch=$(mktemp -d "${TMPDIR:-/tmp}/dmgez-copy-watch-tlc.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
cd -- "$verification_dir/models"

run_tlc() {
	local scenario=$1
	java -Xmx256m -XX:+UseParallelGC -jar "$jar" \
		-workers 1 -seed 0 -fp 0 -noGenerateSpecTE \
		-metadir "$scratch/$scenario" -config "$scenario.cfg" ExternalCopyWatch.tla
}

expect_witness() {
	local scenario=$1 invariant=$2 action=$3 status=0
	local log=$scratch/$scenario.log
	run_tlc "$scenario" > "$log" 2>&1 || status=$?
	if [[ $status != "$violation_exit" ]] ||
		! grep -Fxq -- "Error: Invariant $invariant is violated." "$log" ||
		! grep -Fq -- "<$action line " "$log"; then
		cat -- "$log" >&2
		printf 'Expected %s to reach %s and violate %s (TLC exit %d)\n' "$scenario" "$action" "$invariant" "$status" >&2
		exit 1
	fi
	cat -- "$log"
}

run_tlc ExternalCopyWatch
expect_witness AllCopiesWitness NoAllRequest RequestNormal
expect_witness OneCopyWitness NoSingleMatchRequest RequestNormal
expect_witness IncumbentCopy FreshCopiesAtRequest RequestNormal
expect_witness StaleCopyRecheck FreshCopiesAtRequest EditInPlace
expect_witness ReusedMount ManagedMountOnly ChangeContext
