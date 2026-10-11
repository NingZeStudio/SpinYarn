#!/bin/bash
set -euo pipefail

SPINYARN_BIN="${SPINYARN_BIN:-./target/release/spinyarn}"
SPINYARN_HOST="${SPINYARN_HOST:-127.0.0.1}"
SPINYARN_PORT="${SPINYARN_PORT:-14523}"
BASE_URL="http://${SPINYARN_HOST}:${SPINYARN_PORT}"
TEST_LOG="$(pwd)/spinyarn-test.log"

# 写管理端点（load / load/local / DELETE）统一用该 token 鉴权：
# 服务端读 SPINYARN_ADMIN_TOKEN，请求带 X-Admin-Token 头；读操作不带头。
ADMIN_TOKEN="${SPINYARN_ADMIN_TOKEN:-test-token-please-change}"
ADMIN_H=(-H "X-Admin-Token: $ADMIN_TOKEN")
# 服务实际使用的映射目录（binary 旁 mappings/），load/local 与 DELETE
# 测试的 staging 文件放这里，cleanup 统一清理。
SERVER_MAPPINGS=""
BIG_BODY_FILE="$(pwd)/.spinyarn-test-bigbody.json"
CONC_DIR="$(pwd)/.spinyarn-conc"

stop_server() {
    if [[ -n "${SPINYARN_PID:-}" ]] && kill -0 "$SPINYARN_PID" 2>/dev/null; then
        kill "$SPINYARN_PID" 2>/dev/null || true
        wait "$SPINYARN_PID" 2>/dev/null || true
    fi
    SPINYARN_PID=""
}

cleanup() {
    stop_server
    rm -f "$TEST_LOG" "$BIG_BODY_FILE"
    rm -rf "$CONC_DIR"
    # 管理端点 / vanilla 全链路测试产生的映射副本绝不能污染映射目录
    for f in "${SERVER_MAPPINGS:-}/9.9.9-del-test.tiny.gz" \
             "${SERVER_MAPPINGS:-}/9.9.9-loadlocal.tiny.gz" \
             "${SERVER_MAPPINGS:-}/vanilla/9.9.9-vanilla-test.txt"; do
        rm -f "$f"
    done
    if [[ -n "${SERVER_MAPPINGS:-}" ]]; then
        rm -rf "${SERVER_MAPPINGS}/stage-test"
    fi
}
trap cleanup EXIT

cargo_version() {
    sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' Cargo.toml | head -n 1
}

start_server() {
    echo "[*] Starting SpinYarn on ${SPINYARN_HOST}:${SPINYARN_PORT} ..."
    rm -f "$TEST_LOG"

    # Mappings ship next to the binary, not embedded. Copy them there if missing.
    local bin_dir
    bin_dir="$(dirname "$SPINYARN_BIN")"
    if [[ -d "mappings" && ! -d "$bin_dir/mappings" ]]; then
        echo "[*] Copying mappings/ to $bin_dir/mappings"
        cp -r mappings "$bin_dir/"
    fi
    SERVER_MAPPINGS="$bin_dir/mappings"

    stop_server

    SPINYARN_PID=$(SPINYARN_ADMIN_TOKEN="$ADMIN_TOKEN" RUST_LOG=error "$SPINYARN_BIN" > "$TEST_LOG" 2>&1 & echo $!)

    # The server auto-increments its port when the configured one is taken.
    # Probe the base port and a range after it to find where it actually bound,
    # so we never talk to an unrelated process listening on the base port.
    # A port is claimed only when /api/v1/openapi.json reports the same
    # version as the local Cargo.toml (fingerprint); a foreign server that
    # happens to answer /health is skipped and scanning continues.
    local want
    want="$(cargo_version)"
    local p
    for offset in {0..30}; do
        p=$((SPINYARN_PORT + offset))
        if curl -sf --max-time 1 "http://${SPINYARN_HOST}:${p}/api/v1/health" >/dev/null 2>&1; then
            local got
            got=$(curl -sf --max-time 2 "http://${SPINYARN_HOST}:${p}/api/v1/openapi.json" 2>/dev/null | jq -r .info.version 2>/dev/null || true)
            if [[ -n "$want" && "$got" == "$want" ]]; then
                BASE_URL="http://${SPINYARN_HOST}:${p}"
                SPINYARN_PORT="$p"
                echo "[✓] Server is ready (PID=${SPINYARN_PID}, port=${p}, version=${got})"
                return 0
            fi
            echo "[i] port ${p} answers health but version '${got:-?}' != Cargo.toml '${want}' (foreign?), trying next"
        fi
        sleep 0.3
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

# Capture a request's body + HTTP status without temp files.
# Sets globals API_BODY / API_CODE. Extra args are appended to curl
# (used for "${ADMIN_H[@]}" on write endpoints; reads pass none).
api_post() {
    local url="$1" json="$2"
    shift 2
    local out
    out=$(curl -s -w '\n%{http_code}' -X POST "$url" \
        -H "Content-Type: application/json" -d "$json" --max-time 30 "$@" || true)
    API_CODE=$(printf '%s' "$out" | tail -n 1)
    API_BODY=$(printf '%s' "$out" | sed '$d')
}

api_get() {
    local url="$1"
    shift
    local out
    out=$(curl -s -w '\n%{http_code}' "$url" --max-time 30 "$@" || true)
    API_CODE=$(printf '%s' "$out" | tail -n 1)
    API_BODY=$(printf '%s' "$out" | sed '$d')
}

api_delete() {
    local url="$1"
    shift
    local out
    out=$(curl -s -w '\n%{http_code}' -X DELETE "$url" --max-time 30 "$@" || true)
    API_CODE=$(printf '%s' "$out" | tail -n 1)
    API_BODY=$(printf '%s' "$out" | sed '$d')
}

# 错误包络形状断言：{success:false, error:{code,message}}
assert_error_envelope() {
    local label="$1" body="$2"
    assert_equals "$label: success == false" "$(echo "$body" | jq -r .success)" "false"
    local code
    code=$(echo "$body" | jq -r .error.code)
    if [[ -z "$code" || "$code" == "null" ]]; then
        echo "[✗] $label: missing error.code"
        echo "  body: $body"
        exit 1
    fi
    echo "[✓] $label (error.code=$code)"
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

test_unknown_mapping_type() {
    echo ""
    echo "=== Test: Unknown mapping_type -> 400 ==="
    api_post "${BASE_URL}/api/v1/deobfuscate" \
        '{"content": "hi", "version": "1.21.4", "mapping_type": "bogus"}'
    assert_equals "unknown mapping_type -> 400" "$API_CODE" "400"
    assert_error_envelope "unknown mapping_type envelope" "$API_BODY"
}

test_admin_required() {
    echo ""
    echo "=== Test: Write Endpoints Require Admin Token ==="
    # 写操作无 X-Admin-Token 头一律 403（未配置 token 时同样 403）
    api_post "${BASE_URL}/api/v1/mappings/load" '{"version": "1.21.4"}'
    assert_equals "load without token -> 403" "$API_CODE" "403"
    assert_error_envelope "load without token envelope" "$API_BODY"

    api_post "${BASE_URL}/api/v1/mappings/load/local" \
        '{"version": "1.21.4", "path": "stage-test/src.tiny.gz"}'
    assert_equals "load/local without token -> 403" "$API_CODE" "403"
    assert_error_envelope "load/local without token envelope" "$API_BODY"

    api_delete "${BASE_URL}/api/v1/mappings/1.21.4"
    assert_equals "delete without token -> 403" "$API_CODE" "403"
    assert_error_envelope "delete without token envelope" "$API_BODY"
}

test_mappings_list() {
    echo ""
    echo "=== Test: List Mappings ==="
    api_get "${BASE_URL}/api/v1/mappings"
    assert_equals "list -> 200" "$API_CODE" "200"
    assert_equals "list success" "$(echo "$API_BODY" | jq -r .success)" "true"
    assert_equals "yarn list contains 1.21.4" \
        "$(echo "$API_BODY" | jq -r '.data.yarn | any(. == "1.21.4")')" "true"
    assert_equals "vanilla list is array" \
        "$(echo "$API_BODY" | jq -r '.data.vanilla | type')" "array"
}

test_mapping_stats() {
    echo ""
    echo "=== Test: Mapping Stats ==="
    api_get "${BASE_URL}/api/v1/mappings/yarn/1.21.4"
    assert_equals "stats -> 200" "$API_CODE" "200"
    assert_equals "stats success" "$(echo "$API_BODY" | jq -r .success)" "true"
    assert_equals "stats version" "$(echo "$API_BODY" | jq -r .data.version)" "1.21.4"
    assert_equals "stats mapping_type" "$(echo "$API_BODY" | jq -r .data.mapping_type)" "yarn"

    api_get "${BASE_URL}/api/v1/mappings/bogus/1.21.4"
    assert_equals "illegal type -> 400" "$API_CODE" "400"
    assert_error_envelope "illegal type envelope" "$API_BODY"

    api_get "${BASE_URL}/api/v1/mappings/yarn/a..b"
    assert_equals "illegal version -> 400" "$API_CODE" "400"
    assert_error_envelope "illegal version envelope" "$API_BODY"

    api_get "${BASE_URL}/api/v1/mappings/yarn/9.9.9-no-such-version"
    assert_equals "missing version -> 404" "$API_CODE" "404"
    assert_error_envelope "missing version envelope" "$API_BODY"
}

test_load_remote() {
    echo ""
    echo "=== Test: Load Mapping (Maven refresh) ==="
    # 对已在本地的版本做 load：带 token 的合法请求应成功（本地命中或远端刷新）
    api_post "${BASE_URL}/api/v1/mappings/load" \
        '{"version": "1.21.4", "mapping_type": "yarn"}' "${ADMIN_H[@]}"
    assert_equals "load ok -> 200" "$API_CODE" "200"
    assert_equals "load success" "$(echo "$API_BODY" | jq -r .success)" "true"

    api_post "${BASE_URL}/api/v1/mappings/load" \
        '{"version": "a..b"}' "${ADMIN_H[@]}"
    assert_equals "load illegal version -> 400" "$API_CODE" "400"
    assert_error_envelope "load illegal version envelope" "$API_BODY"

    api_post "${BASE_URL}/api/v1/mappings/load" \
        '{"version": "1.21.4", "mapping_type": "bogus"}' "${ADMIN_H[@]}"
    assert_equals "load illegal type -> 400" "$API_CODE" "400"
    assert_error_envelope "load illegal type envelope" "$API_BODY"
}

test_load_local() {
    echo ""
    echo "=== Test: load/local ==="
    mkdir -p "${SERVER_MAPPINGS}/stage-test"
    cp "mappings/1.21.4.tiny.gz" "${SERVER_MAPPINGS}/stage-test/src.tiny.gz"

    # 正例：带 token
    api_post "${BASE_URL}/api/v1/mappings/load/local" \
        '{"version": "9.9.9-loadlocal", "mapping_type": "yarn", "path": "stage-test/src.tiny.gz"}' \
        "${ADMIN_H[@]}"
    assert_equals "load/local ok -> 200" "$API_CODE" "200"
    assert_equals "load/local success" "$(echo "$API_BODY" | jq -r .success)" "true"
    assert_equals "load/local version" "$(echo "$API_BODY" | jq -r .data.version)" "9.9.9-loadlocal"
    if [[ ! -f "${SERVER_MAPPINGS}/9.9.9-loadlocal.tiny.gz" ]]; then
        echo "[✗] load/local target file missing"
        exit 1
    fi
    echo "[✓] load/local target file exists"

    # 反例：../escape 穿越
    api_post "${BASE_URL}/api/v1/mappings/load/local" \
        '{"version": "1.21.4", "path": "../escape.tiny.gz"}' "${ADMIN_H[@]}"
    assert_equals "load/local ../escape -> 400" "$API_CODE" "400"
    assert_error_envelope "load/local ../escape envelope" "$API_BODY"

    # 反例：绝对路径
    local abs_src
    abs_src="$(cd "${SERVER_MAPPINGS}" && pwd)/stage-test/src.tiny.gz"
    api_post "${BASE_URL}/api/v1/mappings/load/local" \
        "$(jq -n --arg p "$abs_src" '{version: "1.21.4", path: $p}')" "${ADMIN_H[@]}"
    assert_equals "load/local absolute path -> 400" "$API_CODE" "400"
    assert_error_envelope "load/local absolute path envelope" "$API_BODY"

    # 反例：空 path
    api_post "${BASE_URL}/api/v1/mappings/load/local" \
        '{"version": "1.21.4", "path": ""}' "${ADMIN_H[@]}"
    assert_equals "load/local empty path -> 400" "$API_CODE" "400"
    assert_error_envelope "load/local empty path envelope" "$API_BODY"

    # 清理加载的副本（走 DELETE，顺带覆盖删除路径）
    api_delete "${BASE_URL}/api/v1/mappings/9.9.9-loadlocal" "${ADMIN_H[@]}"
    assert_equals "load/local cleanup delete -> 200" "$API_CODE" "200"
}

test_delete_mapping() {
    echo ""
    echo "=== Test: Delete Mapping ==="
    cp "${SERVER_MAPPINGS}/1.21.4.tiny.gz" "${SERVER_MAPPINGS}/9.9.9-del-test.tiny.gz"

    # 无 token -> 403，且文件仍在
    api_delete "${BASE_URL}/api/v1/mappings/9.9.9-del-test"
    assert_equals "delete without token -> 403" "$API_CODE" "403"
    assert_error_envelope "delete without token envelope" "$API_BODY"
    if [[ ! -f "${SERVER_MAPPINGS}/9.9.9-del-test.tiny.gz" ]]; then
        echo "[✗] delete without token must not remove the file"
        exit 1
    fi
    echo "[✓] delete without token keeps the file"

    # 带 token 删除 -> 200，文件消失
    api_delete "${BASE_URL}/api/v1/mappings/9.9.9-del-test" "${ADMIN_H[@]}"
    assert_equals "delete ok -> 200" "$API_CODE" "200"
    assert_equals "delete success" "$(echo "$API_BODY" | jq -r .success)" "true"
    if [[ -f "${SERVER_MAPPINGS}/9.9.9-del-test.tiny.gz" ]]; then
        echo "[✗] delete did not remove the file"
        exit 1
    fi
    echo "[✓] delete removed the file"

    # 重复删除 -> 404
    api_delete "${BASE_URL}/api/v1/mappings/9.9.9-del-test" "${ADMIN_H[@]}"
    assert_equals "delete again -> 404" "$API_CODE" "404"
    assert_error_envelope "delete again envelope" "$API_BODY"
}

test_openapi() {
    echo ""
    echo "=== Test: OpenAPI ==="
    api_get "${BASE_URL}/api/v1/openapi.json"
    assert_equals "openapi -> 200" "$API_CODE" "200"
    assert_equals "openapi version == Cargo.toml" \
        "$(echo "$API_BODY" | jq -r .info.version)" "$(cargo_version)"
}

test_vanilla_chain() {
    echo ""
    echo "=== Test: Vanilla Full Chain (self-contained fixture) ==="
    # 用仓库自带 vanilla fixture 放入临时版本名，测试前后清理
    mkdir -p "${SERVER_MAPPINGS}/vanilla"
    rm -f "${SERVER_MAPPINGS}/vanilla/9.9.9-vanilla-test.txt"
    cp tests/fixtures/test-mappings-vanilla.tsrg \
        "${SERVER_MAPPINGS}/vanilla/9.9.9-vanilla-test.txt"

    local line
    line=$(grep -m1 'at flk' tests/fixtures/1.21.4-vanilla.log)
    local payload
    payload=$(jq -n --arg c "$line" \
        '{content: $c, version: "9.9.9-vanilla-test", mapping_type: "vanilla"}')
    api_post "${BASE_URL}/api/v1/deobfuscate" "$payload"
    assert_equals "vanilla deobfuscate -> 200" "$API_CODE" "200"
    assert_equals "vanilla success" "$(echo "$API_BODY" | jq -r .success)" "true"
    local text
    text=$(echo "$API_BODY" | jq -r .data.deobfuscated)
    assert_contains "vanilla class mapped" "$text" "net.minecraft.client.Minecraft"

    rm -f "${SERVER_MAPPINGS}/vanilla/9.9.9-vanilla-test.txt"
    echo "[✓] vanilla fixture cleaned up"
}

test_large_body() {
    echo ""
    echo "=== Test: Oversized Body ==="
    # 64MB 上限：构造 65MB JSON（路径走 argv，无注入风险）
    rm -f "$BIG_BODY_FILE"
    python3 - "$BIG_BODY_FILE" <<'PYEOF'
import sys
path = sys.argv[1]
with open(path, "w") as f:
    f.write('{"content": "')
    f.write("a" * (65 * 1024 * 1024))
    f.write('", "version": "1.21.4"}')
PYEOF
    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" --max-time 120 \
        -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        --data-binary "@${BIG_BODY_FILE}" || true)
    rm -f "$BIG_BODY_FILE"
    if [[ "$code" == "413" || "$code" == "400" ]]; then
        echo "[✓] oversized body rejected ($code)"
    else
        echo "[✗] oversized body: expected 413/400, got $code"
        exit 1
    fi
}

test_concurrent_smoke() {
    echo ""
    echo "=== Test: 32 Concurrent Small Deobfuscate ==="
    rm -rf "$CONC_DIR"
    mkdir -p "$CONC_DIR"
    local i
    for i in $(seq 1 32); do
        (
            if curl -sf --max-time 30 -X POST "${BASE_URL}/api/v1/deobfuscate" \
                -H "Content-Type: application/json" \
                -d '{"content": "at net.minecraft.class_7833.method_46349(Test.java:1)", "version": "1.21.4"}' \
                2>/dev/null | grep -q "RotationAxis"; then
                echo ok > "$CONC_DIR/s.$i"
            else
                echo fail > "$CONC_DIR/s.$i"
            fi
        ) &
    done
    wait
    local fails=0
    for i in $(seq 1 32); do
        if [[ "$(cat "$CONC_DIR/s.$i" 2>/dev/null)" != "ok" ]]; then
            fails=$((fails + 1))
        fi
    done
    rm -rf "$CONC_DIR"
    assert_equals "32 concurrent all mapped" "$fails" "0"
}

test_cors_preflight() {
    echo ""
    echo "=== Test: CORS Preflight (informational, no assertion) ==="
    curl -s -D - -o /dev/null -X OPTIONS "${BASE_URL}/api/v1/deobfuscate" \
        -H "Origin: http://example.com" \
        -H "Access-Control-Request-Method: POST" || true
    echo "[i] preflight headers above are for reference only"
}

test_zero_concurrency() {
    echo ""
    echo "=== Test: SPINYARN_MAX_CONCURRENCY=0 Still Serves ==="
    # API 侧 warn + 默认 32：重启带 0，服务必须可用且请求不能挂死
    stop_server
    SPINYARN_MAX_CONCURRENCY=0 start_server
    local resp
    resp=$(curl -sf --max-time 10 "${BASE_URL}/api/v1/health")
    assert_equals "health ok with max_concurrency=0" \
        "$(echo "$resp" | jq -r .data.status)" "healthy"
    resp=$(curl -sf --max-time 30 -X POST "${BASE_URL}/api/v1/deobfuscate" \
        -H "Content-Type: application/json" \
        -d '{"content": "at net.minecraft.class_7833.method_46349(Test.java:1)", "version": "1.21.4"}')
    assert_contains "deobfuscate ok with max_concurrency=0" \
        "$(echo "$resp" | jq -r .data.deobfuscated)" "RotationAxis"
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

    # The stack/deobfuscation tests need real mappings. Fail loudly instead of
    # silently passthrough-ing every class (mappings are no longer committed).
    local missing=0
    for v in 1.21.4 1.14.4; do
        if [[ ! -f "mappings/${v}.tiny.gz" ]]; then
            echo "[✗] Missing mapping: mappings/${v}.tiny.gz"
            missing=1
        fi
    done
    if [[ $missing -eq 1 ]]; then
        echo "    Download required Yarn mappings with:"
        echo "      bash scripts/download_mappings.sh 1.21.4 1.14.4"
        exit 1
    fi

    start_server
    test_health
    test_openapi
    test_deobfuscate_stack
    test_deobfuscate_legacy_version
    test_deobfuscate_descriptor
    test_deobfuscate_multi_line
    test_deobfuscate_unsupported
    test_deobfuscate_plain
    test_invalid_request
    test_unknown_mapping_type
    test_admin_required
    test_mappings_list
    test_mapping_stats
    test_load_remote
    test_load_local
    test_delete_mapping
    test_vanilla_chain
    test_large_body
    test_concurrent_smoke
    test_cors_preflight
    # 重启服务的测试放最后
    test_zero_concurrency

    echo ""
    echo "========================================="
    echo " All tests passed!"
    echo "========================================="
}

main "$@"
