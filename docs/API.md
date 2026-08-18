# SpinYarn 接入文档

SpinYarn 是一个 Minecraft 日志反混淆引擎，将混淆的堆栈追踪（`class_XXX` / `method_XXX` / `field_XXX`，或 Vanilla 短混淆名）转换为可读名称。本文档面向需要接入的开发者，覆盖**两种交付形态**的部署、接口约定与客户端示例：

1. **Web API** —— 独立 HTTP 服务（`spinyarn` 二进制）
2. **C ABI / PHP 扩展** —— 进程内嵌入（`libspinyarn_capi` 共享库，含 `crates/php/` PHP 8 扩展）

两者共享同一反混淆核心（`spinyarn-core`），能力等价；选择依据见下表。

| 维度 | Web API | C ABI / PHP 扩展 |
|------|---------|------------------|
| 形态 | Axum + Tokio 网络服务 | 进程内同步共享库 |
| 部署 | 独立进程，监听端口 | 随宿主进程加载（如 php-fpm） |
| 配置 | `config.toml`（自动生成） | **无配置文件**，构造时位置参数传入 |
| 网络开销 | 有（HTTP + JSON 序列化） | 无（函数调用） |
| 并发控制 | 内置信号量（默认 32） | 宿主自行控制 |
| 全量下载 | 启动时后台自动补全 | `spinyarn_bootstrap` 显式调用 |
| 适合场景 | 多客户端共享服务 | 单一宿主高频嵌入（如 PHP 项目） |

---

# 第一部分：Web API 对接

## 1. 部署

### 1.1 构建

```bash
cargo build --release
```

产物为单文件二进制 `target/release/spinyarn`，映射表**不嵌入二进制**，运行时按需加载。

### 1.2 映射准备

映射文件放在**二进制同级目录的 `mappings/` 子目录**下（基于可执行文件路径定位，与工作目录无关）：

- Yarn：`mappings/<version>.tiny.gz`
- Vanilla：`mappings/vanilla/<version>.txt`

两种获取方式：

1. **启动自动补全（推荐，默认开启）**：`maven.auto_download = true`（默认）时，服务启动后会在后台按 `maven.bootstrap_versions` 清单（默认 1.14 ~ 1.21.11 共 43 个版本的 Yarn + Vanilla 双家族映射）**逐个检查缺失并补全**（目录不存在会自动创建），不阻塞启动。
2. **脚本预下载**：
   ```bash
   bash scripts/download_mappings.sh            # 下载全部 43 个 Yarn 版本
   bash scripts/download_mappings.sh 1.21.9     # 只下载指定版本
   python3 scripts/download_vanilla_mappings.py  # 下载 Vanilla 官方映射
   ```

运行时请求一个本地缺失的 `1.x` 版本（含 `-pre`/`-rc`）时，若 `auto_download` 开启，也会按需自动下载（磁盘 TTL 7 天）。

### 1.3 配置

配置文件查找顺序：二进制同级 `config.toml` → 当前目录 `config.toml` → `SpinYarn.toml` → `/etc/spinyarn/config.toml`。**均不存在时，首次启动会在二进制同级自动生成默认 `config.toml`**。

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
# 首次启动会自动：生成 config.toml（若缺失）+ 创建并补全 mappings/（若缺失）
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

# 第二部分：C ABI 对接

`libspinyarn_capi` 是 `spinyarn-core` 的 C 接口（cdylib + staticlib），供任何支持 FFI 的宿主语言（C/C++/PHP/其他）进程内嵌入。所有函数 `catch_unwind` 防 panic 跨 FFI 边界，NULL 参数安全。

## 1. 构建

```bash
cargo build --release -p spinyarn-capi
# 产物：
#   target/release/libspinyarn_capi.so   （动态库）
#   target/release/libspinyarn_capi.a    （静态库）
# 头文件：crates/capi/include/spinyarn.h
```

## 2. 数据类型

```c
typedef struct spinyarn_handle spinyarn_handle_t;   /* 引擎句柄（不透明） */
typedef struct spinyarn_result spinyarn_result_t;   /* 单次反混淆结果（不透明） */

typedef enum {
    SPINYARN_YARN = 0,     /* Fabric Yarn 映射 */
    SPINYARN_VANILLA = 1,  /* Mojang 官方映射（TSRG） */
} spinyarn_mapping_type_t;
```

## 3. 初始化与释放

### `spinyarn_init` / `spinyarn_init_full`

```c
/* 简版：默认 LRU 缓存（44 条目 / 水位 40/30） */
spinyarn_handle_t *spinyarn_init(const char *mappings_dir, int auto_download);

/* 全参数版（MySQLi 风格位置参数） */
spinyarn_handle_t *spinyarn_init_full(
    const char *mappings_dir,      /* 映射目录；NULL = 用 SPINYARN_MAPPINGS_DIR 或 <exe>/mappings */
    int auto_download,             /* 1 = 缺失版本自动下载，0 = 关闭 */
    size_t cache_max_entries,      /* 0 = 禁用缓存；正数 = 缓存上限 */
    size_t cache_high_watermark,   /* 0 = 自动（随上限推导）；否则用该值 */
    size_t cache_low_watermark     /* 0 = 自动；否则用该值 */
);
```

两者失败均返回 `NULL`。无配置文件，全部设置通过参数传入。

### `spinyarn_free`

```c
void spinyarn_free(spinyarn_handle_t *handle);
```

释放引擎。传入 `NULL` 安全（no-op）。

## 4. 反混淆

### `spinyarn_deobfuscate`

```c
spinyarn_result_t *spinyarn_deobfuscate(
    spinyarn_handle_t *handle,
    const char *content,           /* UTF-8 日志文本 */
    size_t content_len,            /* 显式字节长度（无需 NUL 结尾）；上限 64MB */
    const char *version,           /* Minecraft 版本，如 "1.21.9" */
    spinyarn_mapping_type_t mapping_type
);
```

返回结果（handle 有效时不为 `NULL`）。映射不可用时**原样透传**（计数为 0），不报错。`content_len == 0` 或超过 64MB 上限时返回 `NULL`。

### 结果访问器

```c
const char *spinyarn_result_text(const spinyarn_result_t *result);  /* 反混淆文本（NUL 结尾） */
size_t      spinyarn_result_len(const spinyarn_result_t *result);   /* 文本字节长度 */
size_t      spinyarn_result_classes(const spinyarn_result_t *result);
size_t      spinyarn_result_methods(const spinyarn_result_t *result);
size_t      spinyarn_result_fields(const spinyarn_result_t *result);
double      spinyarn_result_time_ms(const spinyarn_result_t *result);
```

> **注意**：文本若含 NUL 字节会被截断到首个 NUL（真实 MC 日志无 NUL，实际无影响），`spinyarn_result_len` 返回截断后的长度。

### `spinyarn_result_free`

```c
void spinyarn_result_free(spinyarn_result_t *result);
```

释放结果。每个 `spinyarn_deobfuscate` 返回的结果都必须调用此函数释放（`NULL` 安全）。

## 5. 映射管理与全量下载

### `spinyarn_load_mapping`

```c
int spinyarn_load_mapping(
    spinyarn_handle_t *handle,
    const char *version,
    spinyarn_mapping_type_t mapping_type,
    int force                      /* 1 = 强制刷新（忽略 TTL），0 = 仅缺失/过期时下载 */
);
```

返回 `1` = 映射已就绪，`0` = 失败（不可下载或网络错误）。

### `spinyarn_has_mapping`

```c
int spinyarn_has_mapping(
    spinyarn_handle_t *handle,
    const char *version,
    spinyarn_mapping_type_t mapping_type
);
```

返回 `1` = 本地存在该映射文件，`0` = 不存在。

### `spinyarn_bootstrap`

```c
size_t spinyarn_bootstrap(spinyarn_handle_t *handle);
```

**全量下载**默认版本清单（43 Yarn + Vanilla 双家族）：逐个检查缺失并下载，已存在的跳过。**同步阻塞**，应在部署/初始化阶段调用，不要在热请求路径调用。返回下载的文件数（`>= 0`）。

## 6. 版本号

```c
const char *spinyarn_version(void);   /* 如 "1.0.0-pre.1"，静态生命周期 */
```

## 7. 完整 C 示例

```c
#include <stdio.h>
#include <string.h>
#include "spinyarn.h"

int main(void) {
    /* 全参数初始化：映射目录 + 自动下载 + 缓存上限 44 / 水位 40/30 */
    spinyarn_handle_t *h = spinyarn_init_full("./mappings", 1, 44, 40, 30);
    if (!h) {
        fprintf(stderr, "init failed\n");
        return 1;
    }

    /* 部署阶段全量下载默认版本清单 */
    size_t n = spinyarn_bootstrap(h);
    printf("bootstrap downloaded %zu mapping files\n", n);

    const char *log = "at net.minecraft.class_310.method_55608(Client.java:465)\n";
    spinyarn_result_t *r = spinyarn_deobfuscate(
        h, log, strlen(log), "1.21.9", SPINYARN_YARN);
    if (r) {
        printf("%.*s", (int)spinyarn_result_len(r), spinyarn_result_text(r));
        printf("classes=%zu methods=%zu fields=%zu time=%.2fms\n",
               spinyarn_result_classes(r), spinyarn_result_methods(r),
               spinyarn_result_fields(r), spinyarn_result_time_ms(r));
        spinyarn_result_free(r);
    }

    spinyarn_free(h);
    return 0;
}
```

编译（链接动态库）：

```bash
gcc example.c -I crates/capi/include -L target/release -lspinyarn_capi -o example
# 运行需 LD_LIBRARY_PATH 指向 target/release
```

---

# 第三部分：PHP 8 扩展对接

`crates/php/` 提供 PHP 8（NTS）扩展 `spinyarn.so`，是 C ABI 的薄封装。engine 句柄以 PHP resource 表示，**析构自动释放**（无需手动 free）。

## 1. 构建与加载

先构建 C ABI 库，再构建 PHP 扩展。

### 1.1 用 phpize 构建（推荐）

```bash
cd crates/php
phpize
./configure --enable-spinyarn --with-spinyarn-libdir=/path/to/target/release
make
```

### 1.2 无 phpize 手动编译

```bash
cd crates/php
gcc -shared -fPIC -O2 -DCOMPILE_DL_SPINYARN $(php-config --includes) \
  -I ../capi/include spinyarn.c -o spinyarn.so \
  -L ../../target/release -lspinyarn_capi
```

### 1.3 加载

```ini
; php.ini
extension=/path/to/spinyarn.so
```

运行时需让系统找到 `libspinyarn_capi.so`：

```bash
LD_LIBRARY_PATH=/path/to/target/release php ...    # 或 export LD_LIBRARY_PATH
```

## 2. 常量

| 常量 | 值 | 含义 |
|------|----|------|
| `SPINYARN_YARN` | 0 | Fabric Yarn 映射（默认） |
| `SPINYARN_VANILLA` | 1 | Mojang 官方映射（TSRG） |

## 3. 函数

### `spinyarn_init` —— 初始化引擎

```php
spinyarn_init(
    ?string $mappings_dir = null,      // 映射目录；null = SPINYARN_MAPPINGS_DIR 或宿主 exe 旁 ./mappings
    bool $auto_download = true,        // 缺失版本自动下载
    int $cache_max_entries = 44,       // 0 = 禁用缓存；正数 = 缓存上限
    int $cache_high_watermark = 40,    // 0 = 自动
    int $cache_low_watermark = 30      // 0 = 自动
): resource|false
```

返回 resource 句柄，失败返回 `false`。**无配置文件**，MySQLi 风格位置参数。

```php
$h = spinyarn_init(__DIR__ . '/mappings', true);          // 默认缓存 44/40/30
$h = spinyarn_init(__DIR__ . '/mappings', true, 10, 8, 5); // 自定义水位
$h = spinyarn_init(__DIR__ . '/mappings', true, 0);        // 禁用缓存
```

### `spinyarn_deobfuscate` —— 反混淆

```php
spinyarn_deobfuscate(
    $handle,                       // resource，来自 spinyarn_init
    string $content,               // 日志文本（支持多行）
    string $version,               // Minecraft 版本，如 "1.21.9"
    int $mapping_type = SPINYARN_YARN
): array|false
```

成功返回关联数组；映射不可用时**透传**（原样返回、计数为 0）；无效句柄/失败返回 `false`。

```php
$r = spinyarn_deobfuscate($h, $log, '1.21.9', SPINYARN_YARN);
/*
$r === [
    'deobfuscated'   => '...',
    'classes_mapped' => 1,
    'methods_mapped' => 1,
    'fields_mapped'  => 0,
    'total_time_ms'  => 0.02,
]
*/
```

### `spinyarn_load_mapping` —— 加载/刷新映射

```php
spinyarn_load_mapping(
    $handle,
    string $version,
    int $mapping_type = SPINYARN_YARN,
    bool $force = false           // true = 强制刷新（忽略 7 天 TTL）
): bool
```

### `spinyarn_has_mapping` —— 查询映射是否本地存在

```php
spinyarn_has_mapping($handle, string $version, int $mapping_type = SPINYARN_YARN): bool
```

### `spinyarn_bootstrap` —— 全量下载默认版本清单

```php
spinyarn_bootstrap($handle): int|false
```

**同步阻塞**，逐个下载缺失的 43 Yarn + Vanilla 映射，返回下载文件数（`>= 0`）。应在部署/初始化阶段调用一次，**不要**在每次请求里调用。

### `spinyarn_version` —— 库版本

```php
spinyarn_version(): string   // 如 "1.0.0-pre.1"
```

## 4. 完整 PHP 示例

```php
<?php
// 1. 初始化（部署阶段执行一次）
$handle = spinyarn_init(__DIR__ . '/mappings', true, 44, 40, 30);
if ($handle === false) {
    throw new RuntimeException('spinyarn_init failed');
}

// 2. 全量下载默认版本清单（仅部署/初始化阶段调用一次，阻塞）
$downloaded = spinyarn_bootstrap($handle);
echo "bootstrap downloaded {$downloaded} mapping files\n";

// 3. 运行时反混淆（每次请求调用）
$log = "at net.minecraft.class_310.method_55608(Client.java:465)\n";
$r = spinyarn_deobfuscate($handle, $log, '1.21.9', SPINYARN_YARN);
if ($r === false) {
    throw new RuntimeException('deobfuscate failed');
}
echo $r['deobfuscated'];
// 无需手动释放 $handle，resource 析构自动 free
```

## 5. 部署注意

- **PHP-FPM 多进程**：每个 worker 独立进程、独立缓存。用 `$cache_max_entries` 控制单 worker 内存（44 条目 ≈ 300-400MB），worker 多时可调小或传 `0` 禁用。
- **映射目录**：`$mappings_dir` 建议用绝对路径（如 `__DIR__ . '/mappings'`）。传 `null` 会落到宿主 php-fpm 可执行文件旁的 `./mappings`，通常不是期望位置。
- **`LD_LIBRARY_PATH`**：`spinyarn.so` 链接 `libspinyarn_capi.so`，运行时需能找到该动态库。
- **生命周期**：句柄是 resource，脚本结束自动释放；长驻进程（如 Swoole/常驻框架）应复用句柄，避免反复 `spinyarn_init` 触发缓存重建。

---

# 附录：性能与限制

| 项 | 指标 |
|----|------|
| 单版本解析（gzip + 解析） | ~110ms |
| 5MB 日志反混淆 | ~30-100ms（引擎），单请求总耗时 ~360-460ms |
| 峰值内存 | 单版本解析表 ~10MB，缓存水位 30~40 条目 ≈ 300~400MB |
| 请求体上限 | Web API 64MB；C ABI `content_len` 64MB |

**已知限制**：

- Yarn 映射约 1/3 的方法/字段未命名（named 列即 `method_XXX` 自身），无法反混淆；类命名基本完整（0.5% 未命名）。
- Vanilla 短混淆字段名非全局唯一，无法安全反混淆，`fields_mapped` 恒为 0。
- 26.x 版本无混淆、快照 `25wxx` 无 Yarn 映射，均原样透传。

---

## 许可证

本项目使用 **MIT NoFree License**：基于 MIT，但**禁止商用**（非商业、个人、教育、研究用途免费）。详见 [LICENSE](../LICENSE)。
