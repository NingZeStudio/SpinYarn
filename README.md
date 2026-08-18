# SpinYarn

Rust 编写的 Minecraft 日志反混淆引擎，以两个独立构建产物交付：

1. **`spinyarn`**——Web API 服务（Axum + Tokio）
2. **`libspinyarn_capi`**——C ABI 共享库，供 PHP 等宿主语言嵌入（含 `crates/php/` PHP 8 扩展）

利用 Fabric Yarn 映射表，将混淆堆栈追踪（`class_XXX` / `method_XXX` / `field_XXX`）转换为可读名称。作为 LogShare 的替代反混淆层。

## 特性

- **映射外置部署**：43 个版本（1.14 ~ 1.21.11）的 Yarn 映射放在**二进制同级 `./mappings/`** 目录（不嵌入二进制，~6MB），首次启动自动创建并补全
- **自动下载**：请求的 `1.x` 版本不在本地时自动从对应源下载映射（落盘缓存 7 天），新版本/预发布无需改代码
- **启动引导补全**：`auto_download` 开启时，后台按 `maven.bootstrap_versions` 清单（默认 1.14~1.21.11 共 43 个版本的 Yarn + Vanilla 双家族映射，可配置）**逐个检查缺失并补全**（无官方映射的版本自动跳过），不阻塞启动
- **双映射类型**：`mapping_type` 参数支持 Fabric（`yarn`，默认）与 Vanilla（`vanilla`，Mojang official mappings，含行号定位重载）
- **LRU 热门缓存**：`[cache]` 段启用有界 LRU（默认 44 条目 + 水位线 40/30，共享缓存池），热版本命中跳过加载（~6ms）；实测水位 30~40 条目 ≈ 300~400MB；命中/驱逐/条目数经 `/health` 暴露
- **无缓存模型**：按请求版本加载映射、反混淆、用完即弃，单版本解析表 ~10MB，不随请求版本数增长
- **并发限流**：`server.max_concurrency`（默认 32）信号量把峰值内存钉在 N×单版本，突发流量 OOM 换成短暂排队
- **高性能**：手写 memchr 堆栈解析 + 预编译正则兜底（带 memchr 快速过滤，无键行零成本直通），真实 5MB 日志引擎处理 ~30ms
- **模块前缀处理**：`knot/`、`knot//` 模块前缀、嵌套类、源文件名、描述符、`(Native Method)`/`(Unknown Source)` 全覆盖
- **嵌套类裸键**：`class_7512` 这类缺外层的嵌套键通过反向索引解析为 `DimensionType$MonsterSettings`
- **透传机制**：不支持的版本原样返回，不报错
- **纯文本输出**：`/api/v1/deobfuscate/plain` 直接返回 `text/plain` 完整反混淆日志，免 JSON 转义

## 快速开始

### 构建（Web API）

```bash
# 编译 Web API（~6MB 二进制）
cargo build --release
```

### 构建（C ABI 共享库）

```bash
# 编译 C ABI 库：libspinyarn_capi.so（+ .a 静态库）
cargo build --release -p spinyarn-capi
# 头文件：crates/capi/include/spinyarn.h
```

### PHP 扩展（crates/php/）

PHP 8（NTS）。用 phpize 构建：

```bash
cd crates/php
phpize && ./configure --enable-spinyarn --with-spinyarn-libdir=../../target/release
make
```

无 phpize/autoconf 环境可手动编译：

```bash
cd crates/php
gcc -shared -fPIC -O2 -DCOMPILE_DL_SPINYARN $(php-config --includes) \
  -I ../capi/include spinyarn.c -o spinyarn.so \
  -L ../../target/release -lspinyarn_capi
```

加载与调用（运行时需 `LD_LIBRARY_PATH` 指向 cdylib）：

```php
// 无需配置文件：直接传映射目录 + 是否自动下载（+ 可选缓存上限）
$handle = spinyarn_init(__DIR__ . '/mappings', true);        // 默认缓存
$handle = spinyarn_init(__DIR__ . '/mappings', true, 0);     // 禁用 LRU 缓存
$r = spinyarn_deobfuscate($handle, $log, '1.21.9', SPINYARN_YARN);
// $r = ['deobfuscated' => ..., 'classes_mapped' => 1, 'methods_mapped' => 1, ...]
```

PHP 函数：`spinyarn_init` / `spinyarn_deobfuscate` / `spinyarn_load_mapping` / `spinyarn_has_mapping` / `spinyarn_version`；常量 `SPINYARN_YARN` / `SPINYARN_VANILLA`。handle 是 PHP resource，析构自动释放。完整签名见 `crates/php/spinyarn.stub.php`。

### 部署与运行

```bash
# 直接运行即可，无需预先准备映射或配置文件：
./target/release/spinyarn
# 首次启动会自动：
#   1. 在二进制同级生成 config.toml（若不存在）
#   2. 自动创建 mappings/ 并按 bootstrap_versions 清单逐个补全缺失的映射（Yarn + Vanilla）
# 默认监听 127.0.0.1:14523；端口被占用时自动 +1 递增直至找到空闲端口
```

映射目录默认 = **二进制同级 `./mappings/`**（基于可执行文件路径定位，不依赖工作目录），可用 `maven.mappings_dir` 或 `SPINYARN_MAPPINGS_DIR` 覆盖。配置文件 `config.toml` 同样优先从二进制同级目录查找（不存在时自动生成默认配置）。

配置可通过 `config.toml`（`server.host`/`server.port`/`server.max_body_size`/`server.max_concurrency`/`maven.mappings_dir`/`maven.auto_download`/`maven.bootstrap_versions`/`cache.*`）配置；`server.max_body_size`/`server.max_concurrency`/`maven.mappings_dir` 未配置时分别由环境变量 `SPINYARN_MAX_CONCURRENCY`/`SPINYARN_MAPPINGS_DIR` 兜底。

```toml
[server]
host = "127.0.0.1"
port = 14523
max_body_size = 67108864   # 64MB，默认
max_concurrency = 32       # 默认

[maven]
mappings_dir = "./mappings"
auto_download = true       # 缺失版本自动从 Fabric Maven 下载
# bootstrap_versions = ["1.21.9", "1.21.11"]  # 启动引导下载清单（默认 1.14~1.21.11 共 43 个 Yarn 版本）

[cache]
enabled = true             # 有界 LRU 缓存
max_entries = 44           # 条目上限
high_watermark = 40        # 触发批量淘汰
low_watermark = 30         # 淘汰到该水位
```

## API

### `POST /api/v1/deobfuscate`

请求体上限 64MB。

```json
{
  "content": "at net.minecraft.class_310.method_55608(Client.java:465)",
  "version": "1.21.9",
  "mapping_type": "yarn"
}
```

`mapping_type`：`yarn`（默认，Fabric）/ `vanilla`（Mojang official，处理短混淆名堆栈，如 `at fda.o(SourceFile.java:14)`）。

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

行为：
- 版本映射本地可用（或 `auto_download` 可下载）→ 正常反混淆
- 否则 → **原样透传**（`success: true`，计数为 0）

### `POST /api/v1/deobfuscate/plain`

请求体与 `/api/v1/deobfuscate` 相同，但成功时直接返回 `text/plain; charset=utf-8` 的完整反混淆日志（免 JSON 转义，大日志更省流量）；失败时仍返回 JSON 错误结构。

```bash
curl -X POST /api/v1/deobfuscate/plain \
  -H 'Content-Type: application/json' \
  -d '{"content": "at net.minecraft.class_310.method_55608(Client.java:465)", "version": "1.21.9"}'
# 响应体（text/plain）：
# at net.minecraft.client.MinecraftClient.method_55608(MinecraftClient.java:465)
```

### `GET /api/v1/health`

```json
{ "success": true, "data": { "status": "healthy", "uptime_seconds": 123 } }
```

## 映射管理

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/v1/mappings/load` | 从 Maven/Mojang meta 拉取或刷新指定版本映射（`{version, mapping_type, refresh?}`） |
| POST | `/api/v1/mappings/load/local` | 从本地路径加载映射（相对 `mappings/` 目录，防路径穿越） |
| GET | `/api/v1/mappings` | 列出已缓存映射版本（按 yarn/vanilla 分组） |
| GET | `/api/v1/mappings/{type}/{version}` | 查看某版本映射统计（类/方法/字段数） |
| DELETE | `/api/v1/mappings/{version}` | 卸载某版本映射（删除本地文件 + 缓存条目） |

```bash
curl -X POST /api/v1/mappings/load -H 'Content-Type: application/json' \
  -d '{"version":"1.21.4","mapping_type":"vanilla","refresh":true}'
```

完整接口规范见 `GET /api/v1/openapi.json`（OpenAPI 3.0）。

## 支持版本

**无硬编码版本清单**：运行时可反混淆的版本 = 外部映射目录 `mappings/<version>.tiny.gz`（含 `auto_download` 自动下载的版本）。没有映射的版本**原样透传**（`success: true`，计数为 0）。往映射目录新增版本文件（含 pre-release）无需改代码即自动生效。

## 性能（Termux arm64，release 实测）

| 场景 | 耗时 |
|------|------|
| 单版本解析（gzip + 解析，固定成本） | ~110ms |
| 反混淆真实结构 5MB 日志（引擎，快速过滤后） | ~30ms |
| 反混淆纯混淆 5MB 日志（引擎） | ~100ms |
| 单请求总耗时（5MB，含加载+传输） | ~360-460ms |
| 峰值内存 | ~30-40MB |

## 测试

```bash
cargo test --workspace # 单元测试（core/capi/bin 各 crate + 集成 tests/）
bash test.sh           # 集成场景（需先构建 release 二进制）
cargo bench            # 基准测试（引擎吞吐：堆栈/非堆栈/真实/5MB 噪声）
```

真实日志样本：`tests/fixtures/1.21.9-crash.log`、`tests/fixtures/1.21.11-fcl.log.txt`。

## 架构

```
Cargo workspace
├── spinyarn (binary)          # Axum Web API
│     POST /api/v1/deobfuscate        # JSON 响应（deobfuscated + stats）
│     POST /api/v1/deobfuscate/plain  # text/plain 响应
│       → spawn_blocking（受并发限流信号量约束）→ spinyarn_core
├── crates/core                # spinyarn-core（纯 Rust 同步库）
│     Spinyarn 门面：deobfuscate/load_mapping/has_mapping
│       → load_mappings(version)  # 外部目录 → 自动下载 → 透传
│       → 解析 v1/v2 → 4 张全局 HashMap（classes/methods/fields/nested）
│       → LineEngine / VanillaEngine
├── crates/capi                # spinyarn-capi（cdylib，C ABI）
└── crates/php                 # PHP 8 扩展（链接 libspinyarn_capi）
```

详见 `AGENTS.md`（维护约定）。

## 许可

本项目使用 **MIT NoFree License**：基于 MIT，但**禁止商用**（非商业、个人、教育、研究用途免费）。详见 [LICENSE](LICENSE)。
