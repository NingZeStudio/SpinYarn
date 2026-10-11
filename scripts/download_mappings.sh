#!/bin/bash
set -euo pipefail

MAPPINGS_DIR="$(cd "$(dirname "$0")/.." && pwd)/mappings"
MAVEN_BASE="https://maven.fabricmc.net/net/fabricmc/yarn"
TMP_DIR="${MAPPINGS_DIR}/.tmp"
mkdir -p "$MAPPINGS_DIR" "$TMP_DIR"

# Version tokens must mirror core's is_valid_version
# (crates/core/src/mapping/local.rs): non-empty, <=64 chars, first char
# alphanumeric, rest [0-9A-Za-z._-], no ".." (blocks path traversal).
is_valid_version() {
    local v="$1"
    [[ -n "$v" ]] || return 1
    (( ${#v} <= 64 )) || return 1
    [[ "$v" != *..* ]] || return 1
    [[ "$v" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] || return 1
    return 0
}

# Verify a downloaded file against Maven's published <artifact>.sha1.
# Skips when the .sha1 is absent (older artifacts) or sha1sum is missing;
# fails closed (removes the file) on mismatch.
verify_sha1_url() {
    local file="$1" sha1_url="$2"
    command -v sha1sum >/dev/null 2>&1 || return 0
    local expected
    expected=$(curl -sfL --max-time 60 "$sha1_url" 2>/dev/null | awk '{print $1}')
    if [[ -z "${expected:-}" ]]; then
        return 0
    fi
    local actual
    actual=$(sha1sum "$file" | awk '{print $1}')
    if [[ "$actual" != "$expected" ]]; then
        echo "FAILED (sha1 mismatch: got $actual want $expected)"
        rm -f "$file"
        return 1
    fi
    return 0
}

VERSIONS=(
    # 1.14 (5)
    "1.14" "1.14.1" "1.14.2" "1.14.3" "1.14.4"
    # 1.15 (3)
    "1.15" "1.15.1" "1.15.2"
    # 1.16 (6)
    "1.16" "1.16.1" "1.16.2" "1.16.3" "1.16.4" "1.16.5"
    # 1.17 (2)
    "1.17" "1.17.1"
    # 1.18 (3)
    "1.18" "1.18.1" "1.18.2"
    # 1.19 (5)
    "1.19" "1.19.1" "1.19.2" "1.19.3" "1.19.4"
    # 1.20 (7)
    "1.20" "1.20.1" "1.20.2" "1.20.3" "1.20.4" "1.20.5" "1.20.6"
    # 1.21 (12)
    "1.21" "1.21.1" "1.21.2" "1.21.3" "1.21.4" "1.21.5"
    "1.21.6" "1.21.7" "1.21.8" "1.21.9" "1.21.10" "1.21.11"
)

find_latest_build() {
    local encoded="$1"
    local listing
    listing=$(curl -sf "https://maven.fabricmc.net/net/fabricmc/yarn/" 2>/dev/null) || { echo ""; return 1; }
    # Escape regex metacharacters (e.g. '.' in "1.21.4") before interpolating
    # into the grep pattern so the version matches literally.
    local re_encoded
    re_encoded=$(printf '%s' "$encoded" | sed 's/[][\\.^$*+?{}()|]/\\&/g')
    echo "$listing" | grep -o "${re_encoded}%2Bbuild\.[0-9]*" | sed 's/%2B/+/g' \
        | awk -F'build.' 'NR==1{m=$2;l=$0} {if($2>m){m=$2;l=$0}} END{print l}'
}

# Extract .tiny from a JAR, write gzipped output.
# File paths travel via argv (sys.argv), never interpolated into the
# program text, so quotes in paths cannot break out (injection-safe).
jar_to_gz() {
    local jar_file="$1"
    local out_file="$2"
    python3 - "$jar_file" "$out_file" <<'PYEOF'
import gzip
import sys
import zipfile
jar_file, out_file = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(jar_file, 'r') as z:
    for name in z.namelist():
        if name.endswith('.tiny'):
            with gzip.open(out_file, 'wb') as g:
                g.write(z.read(name))
            break
PYEOF
}

download_one() {
    local version="$1"
    local build="$2"
    local gz_file="${MAPPINGS_DIR}/${version}.tiny.gz"

    if [[ -f "$gz_file" ]]; then
        echo "  [skip] $version already exists"
        return 0
    fi

    local encoded=$(echo "$build" | sed 's/\+/%2B/g')
    local dir_url="${MAVEN_BASE}/${encoded}/"
    local listing
    listing=$(curl -sf "$dir_url" 2>/dev/null) || { echo "  [FAIL] $version: no directory"; return 1; }

    echo -n "  [down] $version ($build) ... "

    # 1. Direct tiny.gz (fixed-string match: version dots must not act as regex)
    if echo "$listing" | grep -F -q "yarn-${encoded}-tiny.gz"; then
        if curl -sfL --max-time 300 -o "$gz_file" "${dir_url}yarn-${encoded}-tiny.gz"; then
            if echo "$listing" | grep -F -q "yarn-${encoded}-tiny.gz.sha1"; then
                verify_sha1_url "$gz_file" "${dir_url}yarn-${encoded}-tiny.gz.sha1" || return 1
            fi
            echo "OK ($(du -h "$gz_file" | cut -f1), tiny.gz)"
            return 0
        fi
        echo "FAILED (tiny.gz download)"
        return 1
    fi

    # 2. mergedv2.jar / v2.jar
    local jar_kind=""
    if echo "$listing" | grep -F -q "yarn-${encoded}-mergedv2.jar"; then
        jar_kind="mergedv2"
    elif echo "$listing" | grep -F -q "yarn-${encoded}-v2.jar"; then
        jar_kind="v2"
    fi

    if [[ -n "$jar_kind" ]]; then
        local jar_file="${TMP_DIR}/${version}.jar"
        if curl -sfL --max-time 300 -o "$jar_file" "${dir_url}yarn-${encoded}-${jar_kind}.jar"; then
            if echo "$listing" | grep -F -q "yarn-${encoded}-${jar_kind}.jar.sha1"; then
                verify_sha1_url "$jar_file" "${dir_url}yarn-${encoded}-${jar_kind}.jar.sha1" || { rm -f "$jar_file"; return 1; }
            fi
            if jar_to_gz "$jar_file" "$gz_file" && [[ -f "$gz_file" ]] && [[ -s "$gz_file" ]]; then
                rm -f "$jar_file"
                echo "OK ($(du -h "$gz_file" | cut -f1), $jar_kind)"
                return 0
            fi
            rm -f "$jar_file"
            echo "FAILED (extract)"
            return 1
        fi
        rm -f "$jar_file"
        echo "FAILED (jar download)"
        return 1
    fi

    echo "FAILED (no mapping artifact)"
    return 1
}

process_version() {
    local version="$1"
    local build

    if ! is_valid_version "$version"; then
        echo "  [FAIL] $version: invalid version token (allowed: [0-9A-Za-z._-], <=64 chars, no '..')"
        return 1
    fi

    if [[ -f "${MAPPINGS_DIR}/${version}.tiny.gz" ]]; then
        echo "  [skip] $version already exists"
        return 2
    fi

    build=$(find_latest_build "$(echo "$version" | sed 's/\+/%2B/g')")
    if [[ -z "$build" ]]; then
        echo "  [FAIL] $version: no build found on Maven"
        return 1
    fi

    if download_one "$version" "$build"; then
        return 0
    fi
    return 1
}

# CLI: pass versions to fetch only those; default is the bundled list.
if [[ $# -gt 0 ]]; then
    TARGETS=("$@")
else
    TARGETS=("${VERSIONS[@]}")
fi

echo "=== SpinYarn Mapping Downloader ==="
echo "Target: ${#TARGETS[@]} versions"
echo "Output: $MAPPINGS_DIR"
echo ""

success=0
failed=0
skipped=0

for version in "${TARGETS[@]}"; do
    process_version "$version"
    rc=$?
    if (( rc == 0 )); then
        (( success += 1 ))
    elif (( rc == 2 )); then
        (( skipped += 1 ))
    else
        (( failed += 1 ))
    fi
done

rm -rf "$TMP_DIR"
echo ""
echo "=== Done: $success ok, $failed failed, $skipped skipped ==="