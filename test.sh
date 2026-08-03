#!/bin/bash
set -euo pipefail

SPINYARN_BIN="${SPINYARN_BIN:-./target/release/spinyarn}"
SPINYARN_HOST="${SPINYARN_HOST:-127.0.0.1}"
SPINYARN_PORT="${SPINYARN_PORT:-14523}"
BASE_URL="http://${SPINYARN_HOST}:${SPINYARN_PORT}"
TEST_LOG="$(pwd)/spinyarn-test.log"

cleanup() {
    if [[ -n "${SPINYARN_PID:-}" ]] && kill -0 "$SPINYARN_PID" 2>/dev/null; then
        kill "$SPINYARN_PID" 2>/dev/null || true
        wait "$SPINYARN_PID" 2>/dev/null || true
    fi
    rm -f "$TEST_LOG"
}
trap cleanup EXIT

start_server() {
    echo "[*] Starting SpinYarn on ${SPINYARN_HOST}:${SPINYARN_PORT} ..."
    rm -f "$TEST_LOG"
    SPINYARN_PID=$(RUST_LOG=error "$SPINYARN_BIN" > "$TEST_LOG" 2>&1 & echo $!)

    for i in {1..40}; do
        if curl -sf "${BASE_URL}/api/v1/health" >/dev/null 2>&1; then
            echo "[✓] Server is ready (PID=${SPINYARN_PID})"
            return 0
        fi
        sleep 0.5
    done

    echo "[✗] Server failed to start"
    if [[ -f "$TEST_LOG" ]]; then
        cat "$TEST_LOG"
    fi
    exit 1
}

assert_equals() {
    local label="$1" actual="$2" expected="$3"
    if [[ "$actual" == "$expected" ]]; then
        echo "[✓] $label"
    else
        echo "[✗] $label"
        echo "  expected: $expected"
        echo "  actual:   $actual"
        exit 1
    fi
}

assert_contains() {
    local label="$1" text="$2" substr="$3"
    if [[ "$text" == *"$substr"* ]]; then
        echo "[✓] $label"
    else
        echo "[✗] $label"
        echo "  expected to contain: $substr"
        echo "  actual: $text"
        exit 1
    fi
}

test_health() {
    echo ""
    echo "=== Test: Health ==="
    local resp
    resp=$(curl -sf "${BASE_URL}/api/v1/health")
    assert_equals "health.status == healthy" \
        "$(echo "$resp" | jq -r .data.status)" "healthy"
}

test_deobfuscate_stack() {
    echo ""
    echo "=== Test: Deobfuscate Stack Line (bundled 1.21.4) ==="
    local resp
    resp=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_7833.method_46349(Test.java:1)", "version": "1.21.4"}')

    assert_equals "success == true" "$(echo "$resp" | jq -r .success)" "true"
    local text
    text=$(echo "$resp" | jq -r .data.deobfuscated)
    assert_contains "class mapped" "$text" "net.minecraft.util.math.RotationAxis"
}

test_deobfuscate_legacy_version() {
    echo ""
    echo "=== Test: Deobfuscate v1 Mapping (bundled 1.14.4) ==="
    local resp
    resp=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_310.method_1576(Client.java:456)", "version": "1.14.4"}')

    assert_equals "success == true" "$(echo "$resp" | jq -r .success)" "true"
    local text
    text=$(echo "$resp" | jq -r .data.deobfuscated)
    assert_contains "class mapped" "$text" "net.minecraft.client.MinecraftClient"
}

test_deobfuscate_descriptor() {
    echo ""
    echo "=== Test: Deobfuscate Descriptor (residual) ==="
    local resp
    resp=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": "method_46349(Lnet/minecraft/class_7833;)V", "version": "1.21.4"}')

    local text
    text=$(echo "$resp" | jq -r .data.deobfuscated)
    assert_contains "descriptor class mapped" "$text" "Lnet/minecraft/util/math/RotationAxis;"
}

test_deobfuscate_multi_line() {
    echo ""
    echo "=== Test: Deobfuscate Multi-line Log ==="
    local resp
    resp=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": "ERROR: something went wrong\nat net.minecraft.class_7833.method_46349(Test.java:1)\n[00:00:01] done", "version": "1.21.4"}')

    local text
    text=$(echo "$resp" | jq -r .data.deobfuscated)
    assert_contains "non-stack line kept" "$text" "ERROR: something went wrong"
    assert_contains "stack line mapped" "$text" "net.minecraft.util.math.RotationAxis"
}

test_deobfuscate_unsupported() {
    echo ""
    echo "=== Test: Unsupported Version Passthrough ==="
    local resp
    resp=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_7833", "version": "1.13.2"}')

    assert_equals "passthrough.success == true" "$(echo "$resp" | jq -r .success)" "true"
    assert_equals "deobfuscated == input" \
        "$(echo "$resp" | jq -r .data.deobfuscated)" "at net.minecraft.class_7833"
    assert_equals "classes_mapped == 0" \
        "$(echo "$resp" | jq -r .data.stats.classes_mapped)" "0"
}

test_deobfuscate_plain() {
    echo ""
    echo "=== Test: Deobfuscate Plain Text ==="
    local ctype
    ctype=$(curl -s -o /dev/null -w "%{content_type}" \
        -X POST "${BASE_URL}/api/v1/deobfuscate/plain" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_7833.method_46349(Test.java:1)", "version": "1.21.4"}')
    assert_equals "content-type == text/plain" "$ctype" "text/plain; charset=utf-8"

    local body
    body=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate/plain" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_7833.method_46349(Test.java:1)", "version": "1.21.4"}')
    assert_contains "plain class mapped" "$body" "net.minecraft.util.math.RotationAxis"

    local passthrough
    passthrough=$(curl -sf -X POST "${BASE_URL}/api/v1/deobfuscate/plain" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_7833", "version": "1.13.2"}')
    assert_equals "plain passthrough == input" "$passthrough" "at net.minecraft.class_7833"
}

test_invalid_request() {
    echo ""
    echo "=== Test: Invalid Request ==="
    local http_code
    http_code=$(curl -s -o /dev/null -w "%{http_code}" \
        -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": ""}')
    assert_equals "returns 422 for missing version" "$http_code" "422"
}

main() {
    echo "========================================="
    echo " SpinYarn Integration Test Suite"
    echo "========================================="

    if ! command -v jq &>/dev/null; then
        echo "[✗] jq is required but not installed"
        exit 1
    fi
    if ! command -v curl &>/dev/null; then
        echo "[✗] curl is required but not installed"
        exit 1
    fi
    if [[ ! -x "$SPINYARN_BIN" ]]; then
        echo "[✗] SpinYarn binary not found at: $SPINYARN_BIN"
        echo "    Build with: cargo build --release"
        exit 1
    fi

    start_server
    test_health
    test_deobfuscate_stack
    test_deobfuscate_legacy_version
    test_deobfuscate_descriptor
    test_deobfuscate_multi_line
    test_deobfuscate_unsupported
    test_deobfuscate_plain
    test_invalid_request

    echo ""
    echo "========================================="
    echo " All tests passed!"
    echo "========================================="
}

main "$@"
