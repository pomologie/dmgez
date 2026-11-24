#!/usr/bin/env bash
set -euo pipefail

verification_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
jar=${TLA2TOOLS_JAR:?Set TLA2TOOLS_JAR to the pinned TLA+ Tools v1.8.0 JAR}
jar_dir=$(cd -- "$(dirname -- "$jar")" && pwd)
jar=$jar_dir/$(basename -- "$jar")

expected_sha256=fdffef247f563ef6edbfd44958d2a82db79e7564e07cb673262d896353fa6713
if command -v sha256sum >/dev/null; then
	checksum=$(sha256sum < "$jar")
else
	checksum=$(LC_ALL=C shasum -a 256 < "$jar")
fi
if [[ ${checksum%% *} != "$expected_sha256" ]]; then
	printf '%s\n' 'TLA2TOOLS_JAR does not match the v1.8.0 SHA-256 pin' >&2
	exit 1
fi

run_dir=$(mktemp -d "${TMPDIR:-/tmp}/dmgez-publication.XXXXXXXX")
trap 'rm -rf -- "$run_dir"' EXIT
models=$verification_dir/models
cp -- "$models"/Publication.tla "$models"/Publication.cfg \
	"$models"/PublicationEnospc.cfg "$models"/PublicationRace.cfg "$run_dir/"

tlc_workers=1
tlc_seed=1
tlc_fingerprint=0
invariant_violation_exit=12

check_model() {
	local config=$1 expected_exit=$2 invariant=${3:-} witness=${4:-}
	local log=$run_dir/$config.log tlc_exit=0

	(
		cd -- "$run_dir"
		java -Xmx256m -XX:+UseParallelGC -jar "$jar" \
			-workers "$tlc_workers" -seed "$tlc_seed" -fp "$tlc_fingerprint" \
			-noGenerateSpecTE -metadir "$config-states" -config "$config.cfg" Publication.tla
	) > "$log" 2>&1 || tlc_exit=$?

	if [[ $tlc_exit != "$expected_exit" ]]; then
		printf '%s returned %s instead of %s\n' "$config" "$tlc_exit" "$expected_exit" >&2
		cat -- "$log" >&2
		return 1
	fi
	if [[ -n $invariant ]]; then
		if ! grep -Fxq -- "Error: Invariant $invariant is violated." "$log" ||
			! grep -Fq -- "$witness" "$log"; then
			printf '%s missed its expected counterexample\n' "$config" >&2
			cat -- "$log" >&2
			return 1
		fi
		printf '%s rejected the mutation through %s\n' "$config" "$invariant"
	else
		grep -Fx -- 'Model checking completed. No error has been found.' "$log"
	fi
	grep -E '^TLC2 Version|^[0-9]+ states generated|^The depth of the complete state graph search' "$log"
}

check_model Publication 0
check_model PublicationEnospc "$invariant_violation_exit" CompleteCheckedPublication 'phase |-> "write failed"'
check_model PublicationRace "$invariant_violation_exit" NoClobber 'commits = {WriterA, WriterB}'
