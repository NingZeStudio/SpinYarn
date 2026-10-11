#!/usr/bin/env python3
"""下载 Mojang official mappings（Vanilla client）到 mappings/vanilla/<version>.txt

遍历 version manifest 的 release 版本，下载有 client_mappings 的（1.14.4 ~ 1.21.11，
共 39 个正式版）；已存在则跳过（增量）。26.x 无混淆、1.14.3 及更早无官方映射自动跳过。
用法：python3 scripts/download_vanilla_mappings.py
"""
import hashlib
import json
import os
import re
import sys
import urllib.request

BASE = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "mappings", "vanilla")

# 单文件字节上限（256MB）：先读 MAX_BYTES+1 字节，超限直接报错，避免
# 恶意/异常响应把内存撑爆。
MAX_BYTES = 256 * 1024 * 1024

# 与 core is_valid_version 对齐：首字符字母数字、余下 [0-9A-Za-z._-]、
# 无 ".."、总长 ≤64（vid 会拼进文件名，防路径穿越）。
VERSION_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z._-]{0,63}")


def is_valid_version(vid: str) -> bool:
    return bool(vid) and ".." not in vid and VERSION_RE.fullmatch(vid) is not None


# client_mappings 官方托管域名白名单：manifest 被污染时拒绝向站外发请求。
ALLOWED_HOSTS = {
    "piston-meta.mojang.com",
    "piston-data.mojang.com",
    "launchermeta.mojang.com",
    "launcher.mojang.com",
}


def is_allowed_url(url: str) -> bool:
    try:
        host = urllib.request.urlparse(url).hostname or ""
    except ValueError:
        return False
    return host.lower() in ALLOWED_HOSTS


def main() -> int:
    os.makedirs(OUT, exist_ok=True)
    print(f"输出目录: {os.path.abspath(OUT)}")
    manifest = json.load(urllib.request.urlopen(BASE, timeout=30))
    versions = [v for v in manifest["versions"] if v["type"] == "release"]
    print(f"release 版本 {len(versions)} 个，开始增量下载...")

    ok = skip = fail = 0
    for v in versions:
        vid = v["id"]
        if not is_valid_version(vid):
            print(f"[FAIL] {vid}: 非法版本字符（跳过）")
            fail += 1
            continue
        target = os.path.join(OUT, f"{vid}.txt")
        if os.path.exists(target):
            skip += 1
            continue
        try:
            vj = json.load(urllib.request.urlopen(v["url"], timeout=30))
            cm = vj.get("downloads", {}).get("client_mappings")
            if not cm:
                continue  # 1.14.3 及更早无官方映射 / 26.x 无混淆
            url = cm["url"]
            if not is_allowed_url(url):
                raise ValueError(f"client_mappings URL 不在白名单，拒绝下载：{url}")
            size = (cm.get("size") or 0) // 1024 // 1024
            print(f"[down] {vid} ({size}MB)")
            with urllib.request.urlopen(url, timeout=120) as resp:
                data = resp.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                raise ValueError(f"文件超限（>{MAX_BYTES} 字节），拒绝写入")
            # 版本 JSON 内 downloads.client_mappings 自带 size/sha1：有则必较
            if cm.get("size") is not None and len(data) != cm["size"]:
                raise ValueError(f"size 校验失败：实际 {len(data)}，期望 {cm['size']}")
            if cm.get("sha1"):
                actual = hashlib.sha1(data).hexdigest()
                if actual != cm["sha1"]:
                    raise ValueError(f"sha1 校验失败：实际 {actual}，期望 {cm['sha1']}")
            with open(target, "wb") as f:
                f.write(data)
            ok += 1
        except Exception as e:  # noqa: BLE001
            print(f"[FAIL] {vid}: {e}")
            fail += 1

    print(f"完成: 下载 {ok}, 跳过 {skip}, 失败 {fail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
