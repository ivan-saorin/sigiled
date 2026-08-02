#!/bin/sh
# sync-template.sh — recepimento del template vm-tmpl (design §3, DEC-05/07).
#
# Run from the repo root of a project workspace. Reads the pin
# `template = "vm-tmpl@x.y.z"` from sigiled.toml (fallback: the v1 manifest
# name, DEC-22), fetches that template version, and vendored-replaces the
# SIGILED-owned paths listed in the template's own tools/template-allowlist.
#
#   tools/sync-template.sh                 # sync to the pinned version
#   TEMPLATE_DIR=/path/to/template ...     # use a local template checkout
#   TEMPLATE_REPO=https://... ...          # clone source (default: sigiled repo)
#
# Drift detection: TEMPLATE_VERSION at the repo root records, at every sync,
# the synced version plus a checksum per template-owned file. If any of those
# files was modified locally since (checksum mismatch, or file deleted), the
# sync STOPS and lists them — nothing is overwritten. Resolve by reverting
# the local change (template-owned means the template owns it) or by
# consciously removing the path from the recorded state, then re-run.
#
# Never auto-run (DEC-07): a driver invokes this in a session, reviews the
# diff, and commits via the workspace API. This script never commits.
set -eu

PIN_NAME="" PIN_VER=""
for mf in sigiled.toml mgr.toml; do
    if [ -f "$mf" ]; then
        pin=$(sed -n 's/^template[ ]*=[ ]*"\([^"]*\)"/\1/p' "$mf" | head -1)
        if [ -n "$pin" ]; then
            PIN_NAME="${pin%@*}"; PIN_VER="${pin#*@}"
            break
        fi
    fi
done
[ -n "$PIN_NAME" ] && [ -n "$PIN_VER" ] && [ "$PIN_NAME" != "$PIN_VER" ] || {
    echo "no template pin found (expected: template = \"vm-tmpl@x.y.z\" in sigiled.toml)"; exit 1; }
PIN="$PIN_NAME@$PIN_VER"

# --- locate the template source ---------------------------------------------
CLONE_TMP=""
if [ -n "${TEMPLATE_DIR:-}" ]; then
    SRC="$TEMPLATE_DIR"
else
    REPO="${TEMPLATE_REPO:-https://github.com/ivan-saorin/sigiled.git}"
    CLONE_TMP=$(mktemp -d)
    trap 'rm -rf "$CLONE_TMP"' EXIT
    echo "== fetching template $PIN from $REPO"
    git clone --quiet --depth 1 --branch "$PIN" "$REPO" "$CLONE_TMP" || {
        echo "cannot fetch tag $PIN from $REPO"; exit 1; }
    SRC="$CLONE_TMP/template"
fi
[ -d "$SRC" ] || { echo "template source not found: $SRC"; exit 1; }

ALLOWLIST="$SRC/tools/template-allowlist"
[ -f "$ALLOWLIST" ] || { echo "template has no tools/template-allowlist"; exit 1; }
PATHS=$(grep -v -e '^#' -e '^[ ]*$' "$ALLOWLIST")

# --- drift detection ---------------------------------------------------------
if [ -f TEMPLATE_VERSION ]; then
    drift=""
    while read -r sum path; do
        [ -n "$path" ] || continue
        if [ ! -f "$path" ]; then
            drift="$drift  $path (deleted locally)\n"
        elif [ "$(sha256sum "$path" | cut -d' ' -f1)" != "$sum" ]; then
            drift="$drift  $path (modified locally)\n"
        fi
    done <<EOF
$(tail -n +2 TEMPLATE_VERSION)
EOF
    if [ -n "$drift" ]; then
        echo "== DRIFT on template-owned paths — sync stopped, nothing written:"
        printf "$drift"
        echo "These paths belong to the template (recorded at $(head -1 TEMPLATE_VERSION))."
        echo "Revert the local changes, or consciously drop the path from"
        echo "TEMPLATE_VERSION, then re-run."
        exit 2
    fi
fi

# --- apply -------------------------------------------------------------------
echo "== syncing to $PIN"
{ echo "$PIN"; } > TEMPLATE_VERSION.new
for p in $PATHS; do
    [ -f "$SRC/$p" ] || { echo "allowlisted path missing in template: $p"; rm -f TEMPLATE_VERSION.new; exit 1; }
    mkdir -p "$(dirname "$p")"
    cp "$SRC/$p" "$p"
    chmod --reference="$SRC/$p" "$p" 2>/dev/null || true
    sha256sum "$p" >> TEMPLATE_VERSION.new
    echo "   $p"
done
mv TEMPLATE_VERSION.new TEMPLATE_VERSION
echo "== synced $PIN — review the diff and commit via the workspace API"
