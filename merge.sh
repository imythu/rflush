#!/usr/bin/env bash
# merge.sh - Copy .torrent files from first-level download directories into merge/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MERGE_DIR="$SCRIPT_DIR/merge"
SKIP_DIRS=(".git" ".github" ".idea" ".claude" "src" "target" "frontend" "data" "merge")

mkdir -p "$MERGE_DIR"

should_skip() {
  local name="$1"
  for skip in "${SKIP_DIRS[@]}"; do
    if [[ "$name" == "$skip" ]]; then
      return 0
    fi
  done
  return 1
}

total_copied=0
total_skipped=0

for dir in "$SCRIPT_DIR"/*; do
  [[ -d "$dir" ]] || continue
  name="$(basename "$dir")"
  if should_skip "$name"; then
    continue
  fi

  shopt -s nullglob
  files=("$dir"/*.torrent)
  shopt -u nullglob
  if [[ ${#files[@]} -eq 0 ]]; then
    continue
  fi

  copied=0
  skipped=0
  for f in "${files[@]}"; do
    dest="$MERGE_DIR/$(basename "$f")"
    if [[ -e "$dest" ]]; then
      ((skipped++)) || true
    else
      cp "$f" "$dest"
      ((copied++)) || true
    fi
  done

  total_copied=$((total_copied + copied))
  total_skipped=$((total_skipped + skipped))
  echo "  $name : copied=$copied skipped=$skipped total=${#files[@]}"
done

echo ""
echo "Done. Copied: $total_copied, Skipped (already exist): $total_skipped, Destination: $MERGE_DIR"
