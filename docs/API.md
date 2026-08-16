# SpinYarn 接入文档

SpinYarn 是一个 Minecraft 日志反混淆 Web API 服务，将混淆的堆栈追踪（`class_XXX` / `method_XXX` / `field_XXX`，或 Vanilla 短混淆名）转换为可读名称。本文档面向需要接入该服务的开发者，覆盖部署、接口约定与客户端示例。

## 1. 部署

### 1.1 构建

```bash
cargo build --release
```

产物为单文件二进制 `target/release/spinyarn`（约 6MB），映射表**不嵌入二进制**，运行时按需加载。

### 1.2 映射准备

映射文件放在**二进制同级目录的 `mappings/` 子目录**下（基于可执行文件路径定位，与工作目录无关）：

- Yarn：`mappings/<version>.tiny.gz`
- Vanilla：`mappings/vanilla/<version>.txt`

两种获取方式：

1. **启动自动下载（推荐，默认开启）**：`maven.auto_download = true`（默认）时，若 `mappings/` 目录为空，服务启动后会在后台自动下载 `maven.bootstrap_versions` 清单（默认 1.14 ~ 1.21.11 共 43 个版本的 Yarn + Vanilla 双家族映射），不阻塞启动。
2. **脚本预下载**：
   ```bash
   bash scripts/download_mappings.sh            # 下载全部 43 个 Yarn 版本
   bash scripts/download_mappings.sh 1.21.9     # 只下载指定版本
   python3 scripts/download_vanilla_mappings.py  # 下载 Vanilla 官方映射
   ```

运行时请求一个本地缺失的 `1.x` 版本（含 `-pre`/`-rc`）时，若 `auto_download` 开启，也会按需自动下载（磁盘 TTL 7 天）。

### 1.3 配置

配置文件查找顺序：二进制同级 `config.toml` → 当前目录 `config.toml` → `SpinYarn.toml` → `/etc/spinyarn/config.toml`。均不存在时使用默认值。

```toml
[server]
host = "127.0.0.1"           # 默认
port = 14523                  # 默认；被占用时自动 +1 递增
max_body_size = 67108864      # 64MB，默认
max_concurrency = 32          # 默认；SPINYARN_MAX_CONCURRENCY 可兜底

[maven]
mappings_dir = "./mappings"   # 默认二进制同级；SPINYARN_MAPPINGS_DIR 可兜底
auto_download = true          # 缺失版本自动下载
# bootstrap_versions = ["1.21.9", "1.21.11"]  # 启动引导下载清单（默认 43 个版本）

[cache]
enabled = true                # 有界 LRU 缓存
max_entries = 44
high_watermark = 40
low_watermark = 30
```

### 1.4 启动

```bash
./target/release/spinyarn
# 默认监听 127.0.0.1:14523，端口被占用自动递增
```

---

## 2. 接口约定

### 2.1 基础

- 所有接口前缀：`/api/v1`
- 请求/响应均为 `application/json`（`/deobfuscate/plain` 例外，见下）
- 单次请求体上限 64MB

### 2.2 统一响应包裹

成功：

```json
{ "success": true, "data": { ... } }
```

失败：

```json
{
  "success": false,
  "error": { "code": "BAD_REQUEST", "message": "..." }
}
```

### 2.3 错误码

| HTTP | code | 含义 |
|------|------|------|
| 400 | `BAD_REQUEST` | 参数非法（如非法 version 令牌） |
| 404 | `NOT_FOUND` | 资源不存在（如本地无该映射） |
| 422 | `BAD_REQUEST` | 请求体反序列化失败（如缺 `version`） |
| 500 | `INTERNAL_ERROR` | 服务内部错误（详情只进日志，不外泄） |

---

## 3. 反混淆接口

### 3.1 `POST /api/v1/deobfuscate`

请求：

```json
{
  "content": "at net.minecraft.class_310.method_55608(Client.java:465)",
  "version": "1.21.9",
  "mapping_type": "yarn"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `content` | string | 是 | 待反混淆的日志文本（支持多行） |
| `version` | string | 是 | Minecraft 版本，如 `1.21.9`、`1.18.2-pre1` |
| `mapping_type` | string | 否 | `yarn`（默认）/ `vanilla` |

响应：

```json
{
  "success": true,
  "data": {
    "deobfuscated": "at net.minecraft.client.MinecraftClient.method_55608(MinecraftClient.java:465)",
    "stats": {
      "version": "1.21.9",
      "classes_mapped": 1,
      "methods_mapped": 1,
      "fields_mapped": 0,
      "total_time_ms": 0.03
    }
  }
}
```

**透传行为**：当版本不在本地且无法自动下载时，**不报错**，`success = true` 且 `deobfuscated` 原样返回输入、计数为 0。调用方无需对"不支持的版本"做特殊错误处理。

### 3.2 `POST /api/v1/deobfuscate/plain`

请求体与 `/deobfuscate` 相同，但成功时直接返回 `text/plain; charset=utf-8` 的完整反混淆文本（免 JSON 转义，大日志更省流量）。失败时仍返回 JSON 错误结构。

```bash
curl -X POST http://127.0.0.1:14523/api/v1/deobfuscate/plain \
  -H 'Content-Type: application/json' \
  -d '{"content":"at net.minecraft.class_310.method_55608(Client.java:465)","version":"1.21.9"}'
# 响应体（text/plain）：
# at net.minecraft.client.MinecraftClient.method_55608(MinecraftClient.java:465)
```

### 3.3 映射类型说明

- **`yarn`（默认）**：Fabric Yarn 映射，处理 `class_XXX` / `method_XXX` / `field_XXX` 完整键。方法/字段键全局唯一，可直接查表。
- **`vanilla`**：Mojang 官方映射（TSRG），处理短混淆名（如 `fda.o(SourceFile.java:14)`）。短名非全局唯一，只能走结构化堆栈解析（类确认 + 行号区间定位重载），仅堆栈行可反混淆。

---

## 4. 健康检查

### `GET /api/v1/health`

```json
{
  "success": true,
  "data": {
    "status": "healthy",
    "uptime_seconds": 123,
    "cache": { "enabled": true, "entries": 2, "hits": 10, "misses": 2, "evictions": 0 }
  }
}
```

`cache` 字段在缓存关闭时为 `null`。可用于探活与监控缓存命中率。

---

## 5. 映射管理接口

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/v1/mappings/load` | 从 Maven/Mojang meta 拉取或刷新指定版本映射 |
| POST | `/api/v1/mappings/load/local` | 从本地路径加载映射（相对 `mappings/`，防路径穿越） |
| GET | `/api/v1/mappings` | 列出本地已缓存的映射版本（按 yarn/vanilla 分组） |
| GET | `/api/v1/mappings/{type}/{version}` | 查看某版本映射统计（类/方法/字段数） |
| DELETE | `/api/v1/mappings/{version}` | 卸载某版本映射（删除本地文件 + 缓存条目） |

示例：

```bash
# 拉取/刷新
curl -X POST http://127.0.0.1:14523/api/v1/mappings/load \
  -H 'Content-Type: application/json' \
  -d '{"version":"1.21.4","mapping_type":"vanilla","refresh":true}'

# 列出
curl http://127.0.0.1:14523/api/v1/mappings

# 统计
curl http://127.0.0.1:14523/api/v1/mappings/yarn/1.21.9

# 卸载
curl -X DELETE http://127.0.0.1:14523/api/v1/mappings/1.21.9
```

---

## 6. OpenAPI 规范

完整接口规范见 `GET /api/v1/openapi.json`（OpenAPI 3.0，由 utoipa 生成，版本号与 Cargo.toml 自动同步）。

---

## 7. 客户端接入示例

### 7.1 curl

```bash
curl -X POST http://127.0.0.1:14523/api/v1/deobfuscate \
  -H 'Content-Type: application/json' \
  -d @- <<'EOF'
{"content":"at net.minecraft.class_310.method_55608(Client.java:465)","version":"1.21.9"}
EOF
```

### 7.2 Python

```python
import requests

def deobfuscate(content: str, version: str, mapping_type: str = "yarn") -> str:
    resp = requests.post(
        "http://127.0.0.1:14523/api/v1/deobfuscate",
        json={"content": content, "version": version, "mapping_type": mapping_type},
        timeout=60,
    )
    resp.raise_for_status()
    body = resp.json()
    if not body.get("success"):
        raise RuntimeError(body.get("error"))
    return body["data"]["deobfuscated"]

print(deobfuscate("at net.minecraft.class_310.method_55608(Client.java:465)", "1.21.9"))
```

### 7.3 JavaScript (fetch)

```js
async function deobfuscate(content, version, mappingType = "yarn") {
  const resp = await fetch("http://127.0.0.1:14523/api/v1/deobfuscate", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ content, version, mapping_type: mappingType }),
  });
  const body = await resp.json();
  if (!body.success) throw new Error(body.error?.message);
  return body.data.deobfuscated;
}
```

---

## 8. 性能与限制

| 项 | 指标 |
|----|------|
| 单版本解析（gzip + 解析） | ~110ms |
| 5MB 日志反混淆 | ~30-100ms（引擎），单请求总耗时 ~360-460ms |
| 峰值内存 | 单版本解析表 ~10MB，缓存水位 30~40 条目 ≈ 300~400MB |
| 请求体上限 | 64MB |
| 并发限流 | 默认 32（`server.max_concurrency`） |

**已知限制**：

- Yarn 映射约 1/3 的方法/字段未命名（named 列即 `method_XXX` 自身），无法反混淆；类命名基本完整（0.5% 未命名）。
- Vanilla 短混淆字段名非全局唯一，无法安全反混淆，`fields_mapped` 恒为 0。
- 26.x 版本无混淆、快照 `25wxx` 无 Yarn 映射，均原样透传。

---

## 9. 许可证

本项目使用 **MIT NoFree License**：基于 MIT，但**禁止商用**（非商业、个人、教育、研究用途免费）。详见 [LICENSE](../LICENSE)。
