#!/usr/bin/env bash
# Rebuild an empty-file tree from audiobooks.snapshot so the scanner can walk a
# real directory structure on a machine that has no access to the NAS.
#
# Coverage logic only looks at which files exist and what their extensions are,
# so empty files stand in for the real audio and ebooks without copying any data.
#
# Usage: ./rehydrate.sh [target-dir]
#   target-dir defaults to ./rehydrated next to this script.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
snapshot="$here/audiobooks.snapshot"
target="${1:-$here/rehydrated}"

if [[ ! -f "$snapshot" ]]; then
	echo "snapshot not found: $snapshot" >&2
	exit 1
fi

mkdir -p "$target"
while IFS=$'\t' read -r type path; do
	case "$type" in
	d) mkdir -p "$target/$path" ;;
	f)
		mkdir -p "$target/$(dirname "$path")"
		: >"$target/$path"
		;;
	esac
done <"$snapshot"

echo "Rehydrated $(wc -l <"$snapshot") entries into $target"
