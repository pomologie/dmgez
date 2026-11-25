#!/bin/sh
# Install the CLI from an unpacked dmgez release archive.
set -eu

program=dmgez
checksums=SHA256SUMS
bin_dir=
replace=false

usage() {
	printf '%s\n' "Usage: ./install.sh [--bin-dir DIRECTORY] [--replace]" \
		"Installs the unpacked $program CLI. The default directory is ~/.local/bin."
}

fail() {
	printf '%s\n' "install.sh: $*" >&2
	exit 1
}

while [ "$#" -gt 0 ]; do
	case "$1" in
		--bin-dir)
			[ "$#" -ge 2 ] && [ -n "$2" ] || fail "--bin-dir requires a directory"
			bin_dir=$2
			shift 2
			;;
		--replace)
			replace=true
			shift
			;;
		-h|--help)
			usage
			exit 0
			;;
		*) fail "unknown argument: $1" ;;
	esac
done

if [ -z "$bin_dir" ]; then
	[ -n "${HOME:-}" ] || fail "set HOME or pass --bin-dir"
	bin_dir=$HOME/.local/bin
fi
case "$bin_dir" in
	/*) ;;
	*) bin_dir=$(pwd -P)/$bin_dir ;;
esac

bundle=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
source_binary=$bundle/$program
manifest=$bundle/$checksums
[ -f "$source_binary" ] && [ ! -L "$source_binary" ] &&
	[ -f "$manifest" ] && [ ! -L "$manifest" ] ||
	fail "run this script from an unpacked dmgez release archive"

expected=$(LC_ALL=C awk -v binary="$program" '
	$2 == binary {
		count++
		if (NF == 2 && length($1) == 64 && $1 ~ /^[0-9a-f]+$/)
			digest = $1
	}
	END {
		if (count != 1 || digest == "") exit 1
		print digest
	}
' "$manifest") || fail "expected one SHA-256 entry for $program in $checksums"

if command -v sha256sum >/dev/null 2>&1; then
	hash_binary() { sha256sum < "$1"; }
elif command -v shasum >/dev/null 2>&1; then
	hash_binary() { shasum -a 256 < "$1"; }
else
	fail "sha256sum or shasum is required to verify the release"
fi

verify_binary() {
	actual=$(hash_binary "$1") || fail "could not hash $program"
	[ "${actual%% *}" = "$expected" ] || fail "SHA-256 mismatch for $program"
}

verify_binary "$source_binary"
mkdir -p -- "$bin_dir"
bin_dir=$(CDPATH='' cd -- "$bin_dir" && pwd -P)
destination=$bin_dir/$program
[ ! -L "$destination" ] || fail "destination is a symbolic link: $destination"
if [ -e "$destination" ]; then
	[ -f "$destination" ] || fail "destination is not a regular file: $destination"
	if [ -x "$destination" ] && cmp -s -- "$source_binary" "$destination"; then
		printf 'Already installed at %s\n' "$destination"
		exit 0
	fi
	[ "$replace" = true ] || fail "a different installation exists at $destination; pass --replace to update it"
fi

stage=$(mktemp -d "$bin_dir/.dmgez-install.XXXXXXXX")
staged_binary=$stage/$program
cleanup() {
	rm -f -- "$staged_binary"
	rmdir -- "$stage"
}
trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

cp -- "$source_binary" "$staged_binary"
verify_binary "$staged_binary"
chmod 755 "$staged_binary"
if [ "$replace" = true ] && [ -e "$destination" ]; then
	[ ! -L "$destination" ] && [ -f "$destination" ] ||
		fail "destination is not a regular file: $destination"
	mv -f -- "$staged_binary" "$bin_dir"
else
	ln -- "$staged_binary" "$bin_dir" ||
		fail "could not publish $destination without replacing an existing entry"
fi
printf 'Installed %s\n' "$destination"
case ":${PATH:-}:" in
	*":$bin_dir:"*) ;;
	*) printf 'Add %s to PATH to run %s by name.\n' "$bin_dir" "$program" ;;
esac
