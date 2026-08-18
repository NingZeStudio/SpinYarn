# SpinYarn

Rust 编写的 Minecraft 日志反混淆引擎，以**两个独立构建产物**交付：

1. **`spinyarn`**——Axum Web API 服务（`src/`，Axum + Tokio），HTTP 端点反混淆
2. **`libspinyarn_capi`**——C ABI 共享库（`crates/capi/`，cdylib），供 PHP 等宿主语言嵌入（另有 `crates/php/` PHP 8 扩展）

两者共享同一核心：**`spinyarn-core`**（`crates/core/`，纯 Rust 同步库，无 axum/tokio 依赖）。利用 Fabric Yarn 映射表将混淆堆栈追踪（`class_XXX`/`method_XXX`/`field_XXX`）转换为可读名称。

## 架构要点

### Workspace 结构（Cargo workspace，resolver=2）
```
spinyarn (根 package：Axum Web API binary)
├── src/                 # main.rs + api/（Axum handler）+ error.rs + lib.rs（re-export core）
├── crates/core/         # spinyarn-core：同步核心库（config/cache/mapping/deobfuscator + Spinyarn 门面）
├── crates/capi/         # spinyarn-capi：C ABI（cdylib + staticlib），include/spinyarn.h
└── crates/php/          # PHP 8 扩展（C 源码，config.m4 + spinyarn.c，非 cargo 成员）
```
- **`spinyarn-core`**（无 tokio/axum/utoipa 依赖，纯同步）：`config`/`cache`/`mapping`/`deobfuscator` 四大模块 + **`Spinyarn` 门面类型**（`crates/core/src/lib.rs`）——持有 `mappings_dir`/`auto_download`/可选 LRU cache。**完整流水线** `deobfuscate()`（透传→自动下载→缓存→加载→反混淆，C ABI 用）+ **分步方法**（Web API 用，精确控制信号量门控）：`ensure_available()`/`get_cached()`/`load()`/`insert_cached()`/`deobfuscate_loaded()`，另有 `load_mapping()`/`has_mapping()`/`cache_stats()`/`unload()`/`mappings_dir()`。构造器：`new(&Config)`、`from_settings(dir, auto_download)`、`from_full_settings(dir, auto_download, cache_max, cache_high, cache_low)`（0 = 禁用/自动，MySQLi 风格全参数）。这是 C ABI 与任何嵌入宿主的统一入口
- **`spinyarn-capi`**（`crates/capi/src/lib.rs`）：`#[no_mangle] extern "C"` 函数，全部 `catch_unwind` 防 panic 跨 FFI 边界（UB）。句柄/结果用 `Box::into_raw` 指针 + 显式 free。**无配置文件**——`spinyarn_init(mappings_dir, auto_download)` 简版（默认缓存）与 `spinyarn_init_full(mappings_dir, auto_download, cache_max, cache_high, cache_low)` 全参数版（MySQLi 风格，0 = 禁用/自动）。`spinyarn_deobfuscate` 对 `content_len` 设 64MB 上限防恶意越界读/OOM
- **`spinyarn`（binary）**：`src/lib.rs` 用 `pub use spinyarn_core::{...}` re-export 核心，`AppState` 持 `Arc<Spinyarn>` + `Semaphore`；`src/api/deobfuscate.rs` 复用门面分步方法（缓存命中与 ensure 步骤不占信号量，仅 load 步骤占），与 core 门面逻辑不再重复

- **无缓存模型（基础）+ LRU 缓存（默认开启）**：无缓存时每次调用独立加载映射 → 反混淆 → 释放（单版本解析表实测 ~10MB）。`[cache]` 段启用**有界 LRU**（`crates/core/src/cache.rs`，默认 `max_entries=44`/高水位 40/低水位 30，**共享缓存池**——Yarn/Vanilla 映射同池缓存，key 为 `version+mapping_type`）：解析后表以 `Arc<LoadedMappings>` 缓存，高水位触发批量淘汰至低水位，缓存大小在低~高水位间波动（避免长期满载）；命中请求跳过加载（热请求 ~6ms）且不占并发信号量。实测：缓存水位 30~40 条目 ≈ 300~400MB（服务器内存可承受）；命中/驱逐/条目数经 `/health` 暴露
- **并发限流**（仅 Web API）：`src/api/mod.rs::AppState` 的 `Semaphore`，默认 32（`config.toml` 的 `server.max_concurrency` 可调，未配置时 `SPINYARN_MAX_CONCURRENCY` 环境变量兜底）。无缓存模型下每并发请求持有一整套版本表（~10MB），限流把峰值内存钉在 N×10MB，突发流量 OOM 换成短暂排队（稳态并发约 16，平时不触发）。C ABI 路径无信号量（宿主自行并发控制）
- **自动下载**：`maven.auto_download`（默认 true）时，请求版本不在本地且为 `1.x` 系（含 `-pre`/`-rc`，快照 `25wxx` 与 26.x 排除）→ 按类型自动下载映射落盘（TTL 7 天，过期重下载失败回退旧文件）：Yarn 走 Fabric Maven（`.tiny.gz`），Vanilla 走 Mojang launcher meta（`client.txt` 存 `mappings/vanilla/<version>.txt`），无需改代码即支持新版本。C ABI 同样支持（`Spinyarn::deobfuscate` 内联此逻辑）
- **启动引导补全**（仅 Web API）：`auto_download` 开启时，`main.rs::bootstrap_mappings` 后台任务按 `maven.bootstrap_versions` 清单（默认 = 脚本 `scripts/download_mappings.sh` 的 1.14~1.21.11 共 43 个版本，可在 `config.toml` 覆盖）**逐个检查本地是否缺失，缺哪个补哪个**（`ensure_one` 分别检查 Yarn `<version>.tiny.gz` 与 Vanilla `vanilla/<version>.txt`，存在即跳过），**Yarn 与 Vanilla 双家族都补**（无官方映射的版本如 1.14.3 及更早自动跳过），不阻塞启动、不占请求路径
- **双映射类型**：请求 `mapping_type` 参数（`yarn` 默认 / `vanilla`）；`crates/core/src/mapping/dispatcher.rs` 调度机负责按类型加载与分派引擎（`LineEngine` / `VanillaEngine`）。Vanilla 用 TSRG 解析器（`crates/core/src/mapping/vanilla.rs`），短混淆名只能走结构化堆栈解析（类确认 + TSRG 行号区间定位重载），不适用 residual 正则
- CPU 密集操作（gzip 解压 + 解析 + 反混淆）：Web API 路径放入 `tokio::task::spawn_blocking` 不阻塞 runtime；core/C ABI 路径本就是同步阻塞调用
- **访问日志中间件**：`tower_http::TraceLayer` 记录每个请求的 method/uri/status/耗时。deobfuscate 走 INFO，health 探针走 DEBUG（避免噪音）
- 端点：`POST /api/v1/deobfuscate`（64MB 上限）、`POST /api/v1/deobfuscate/plain`（成功返回 `text/plain`，失败 JSON）、`GET /api/v1/health`；映射管理：`POST /api/v1/mappings/load`（Maven 拉取/刷新）、`POST /api/v1/mappings/load/local`（本地文件加载，路径限 `mappings/` 内防穿越）、`GET /api/v1/mappings`（列出版本）、`GET /api/v1/mappings/{type}/{version}`（统计）、`DELETE /api/v1/mappings/{version}`（卸载文件+缓存）；`GET /api/v1/openapi.json`（utoipa 生成的 OpenAPI 3.0 规范）
- 管理端点（v0.2 曾移除，v0.4 以映射管理形态回归）

## 关键约定

### 内置版本 + 透传
**无硬编码版本清单**：`crates/core/src/mapping/download.rs::is_version_supported` 运行时判断——外部映射目录存在 `<version>.tiny.gz` 即可反混淆，否则 → 若 `maven.auto_download` 开启且版本为 `1.x` 系 → 自动下载；都不行 → **原样透传**（`success: true`，计数为 0），不报错。往映射目录新增版本文件（含 pre-release）无需改代码即自动生效。

### 映射外置（与二进制同级部署）
- **映射不嵌入二进制**：`build.rs`/`embedded.rs` 已移除，binary ~6MB；C ABI 库 ~6MB
- 默认映射目录 = **二进制同级 `./mappings/`**（`std::env::current_exe()` 定位，不依赖工作目录）；`config.toml` 的 `maven.mappings_dir` 或 `SPINYARN_MAPPINGS_DIR` 可覆盖
- **部署即运行，无需预下载映射**：Web API 首次启动自动补全缺失映射（见"启动引导补全"）；C ABI 靠 `auto_download` 按需下载。Release 制品不打包 `mappings/`。如需离线/预下载，可用 `bash scripts/download_mappings.sh [版本...]`
- 加载：外部映射目录存在即用，否则透传
- **C ABI 注意**：`exe_dir()` 在 cdylib 下是宿主进程（如 php-fpm），默认 `mappings/` 会落到宿主可执行文件旁，通常不是期望位置——PHP 侧应通过 `spinyarn_init($mappings_dir, $auto_download)` 显式传映射目录
- **C ABI 部署约定**：C ABI **不读配置文件**，由 PHP 调用方直接传 `spinyarn_init($mappings_dir, $auto_download)`——映射目录通常取 PHP 项目根下的相对路径（如 `__DIR__ . '/mappings'`），自动下载开关显式传入

### 配置加载
`Config::load()` 按顺序查找：二进制同级 `config.toml` → 当前目录 `config.toml` → `SpinYarn.toml` → `/etc/spinyarn/config.toml`，都没找到则**在二进制同级自动生成默认 `config.toml`**（`toml::to_string_pretty` 序列化默认值，写失败仅 warn 不 panic）。配置项：`server.host`/`server.port`/`server.max_body_size`（默认 64MB，无环境变量兜底）/`server.max_concurrency`（默认 32，`SPINYARN_MAX_CONCURRENCY` 兜底）/`maven.mappings_dir`（默认二进制同级 `./mappings`，`SPINYARN_MAPPINGS_DIR` 兜底）/`maven.auto_download`（默认 true）/`maven.bootstrap_versions`（数组，默认 43 个 1.14~1.21.11 Yarn 版本）/`cache.enabled`（默认 true）/`cache.max_entries`（44）/`cache.high_watermark`（40）/`cache.low_watermark`（30）。启动时若端口已被占用，`main.rs` 自动 `port + 1` 递增重试直至找到空闲端口（`u16` 溢出保护）。

### 版本格式兼容
`crates/core/src/mapping/tiny_v2.rs` 自动检测 v1（平铺 `CLASS`/`FIELD`/`METHOD`）和 v2（缩进 `c`/`\tf`/`\tm`）格式，列位置按头部命名空间名定位（兼容 1.14 特殊列序）。

### 反混淆引擎
- 堆栈行：手写 memchr 解析（支持 `knot/`、`java.base/` 前缀、嵌套类回退、源文件名替换）
- 非堆栈行：预编译正则兜底（`crates/core/src/deobfuscator/pattern.rs`），贪婪匹配天然免疫前缀冲突；进正则前先 `contains`（memchr 级）快速过滤无混淆键的行，真实日志大部分行零成本直通
- **嵌套类裸键反向索引**：`Mappings::nested`（`tiny_v2.rs` 构建）把日志中缺外层的裸嵌套键（`class_7512`）解析为完整名（`DimensionType$MonsterSettings`）。仅对全局唯一的内层键建立，150+ 个跨类重复的内层键跳过避免歧义
- 方法/字段键全局唯一（`method_XXXX`/`field_XXXX`），免描述符索引
- **Yarn 数据限制**：约 1/3 方法/字段 named 列即 `method_XXXX` 自身（社区未命名，1.21.9 达 34%），无法反混淆；类命名基本完整（0.5% 未命名）。统计见 `docs/yarn_unmapped_stats.csv`

### C ABI / PHP 扩展
- C 头文件 `crates/capi/include/spinyarn.h`：`spinyarn_init`（mappings_dir 指针 + auto_download 整型，无配置文件，默认缓存）/`spinyarn_init_full`（额外 `cache_max_entries`/`cache_high_watermark`/`cache_low_watermark`，0 = 禁用/自动，MySQLi 风格全参数）/`spinyarn_free`/`spinyarn_deobfuscate`（content 显式长度，64MB 上限，无需 NUL 结尾）/`spinyarn_result_*`（text/len/classes/methods/fields/time_ms）/`spinyarn_load_mapping`/`spinyarn_has_mapping`/`spinyarn_version`。枚举判别值有 `static_assert` 守护（与 Rust `#[repr(C)]` 一致）
- PHP 扩展 `crates/php/`（PHP 8，NTS）：函数 `spinyarn_init(?string $mappings_dir = null, bool $auto_download = true, int $cache_max_entries = 44, int $cache_high_watermark = 40, int $cache_low_watermark = 30)` → resource；`spinyarn_deobfuscate($handle, $content, $version, $mapping_type=SPINYARN_YARN)` → assoc array（`deobfuscated`/`classes_mapped`/`methods_mapped`/`fields_mapped`/`total_time_ms`）；`spinyarn_load_mapping`/`spinyarn_has_mapping`/`spinyarn_version`；常量 `SPINYARN_YARN=0`/`SPINYARN_VANILLA=1`。handle 为 PHP resource，析构自动 `spinyarn_free`（无需手动 free）。另有 `spinyarn.stub.php` 文档化函数签名
- 构建：`config.m4` 用 phpize/autoconf（`SPINYARN_LIBDIR` 指向 `target/release`，缺库/头文件时 `AC_MSG_ERROR` 提前报错）；无 phpize 环境可手动 `gcc -shared -fPIC $(php-config --includes) -I ../capi/include spinyarn.c -o spinyarn.so -L ../../target/release -lspinyarn_capi`，运行需 `LD_LIBRARY_PATH` 指向 cdylib

## 测试

```bash
cargo test --workspace     # 单元测试（tests/ 目录 + 各 crate 内 #[cfg(test)]）
bash test.sh               # 集成测试（需先构建 release 二进制）
cargo bench                # 基准测试（benches/deobfuscate.rs，引擎吞吐）
```

- 单元测试：引擎/解析器边界场景 + **真实日志快照回归**（`tests/snapshot_test.rs`：对 `tests/fixtures/` 真实日志反混淆，与 `tests/snapshots/` 逐字节对比，防引擎行为漂移；引擎有意变更输出时删除/重建对应 `.snap` 文件）
- **快照测试会在映射文件缺失时静默跳过**（`snapshot_test.rs::require_mapping!`），不报错——因此全新 clone 直接 `cargo test` 并非完整跑测。快照回归需 `mappings/` 下存在 `1.21.9`/`1.21.11`/`1.18.2-pre1`/`1.21.3`（Vanilla 快照用自包含 fixture，无需下载）；`test.sh` 额外需 `1.21.4`/`1.14.4`。只下所需版本：`bash scripts/download_mappings.sh 1.21.9 1.21.11 1.18.2-pre1 1.21.3 1.21.4 1.14.4`
- 集成测试依赖 `jq` 和 `curl`。服务端口被占用时自动 +1 递增，不会启动失败。
- C ABI 单元测试在 `crates/capi/src/lib.rs`（`#[cfg(test)]`），覆盖 version/透传/NULL 参数安全；不依赖网络（透传用快照版本）

## CI / Release

- CI（push/PR 到 main/develop）：`cargo check --workspace --all-targets` → `cargo test --workspace --all-targets` → `cargo build --release --workspace` → `bash test.sh`
- Release：commit message 以 `[Build]` 开头时触发多平台构建（binary + cdylib 双产物），Release notes 自动取自 `CHANGELOG.md` 最新版本条目

## 注意事项

- Axum 默认请求体限制 2MB，已通过 `DefaultBodyLimit::max(config.server.max_body_size)`（`src/api/mod.rs::build_router`）放大到 64MB 上限（`crates/core/src/config.rs::DEFAULT_MAX_BODY_SIZE`），不要缩小
- `regex` crate 不支持 look-around，边界处理用 `\b` + 前缀边界捕获组
- OpenAPI 文档版本号在 `src/api/mod.rs::ApiDoc` 的 `info(version=...)` 中为宏字面量（仅作占位），运行时由 `openapi_json` handler 用 `env!("CARGO_PKG_VERSION")` 覆盖，因此更新 `Cargo.toml` 版本无需手动同步
- 修改 core 公共接口时，capi 的 `spinyarn.h` 与 PHP 扩展的 `spinyarn.c` 需同步（函数签名/常量）；C ABI 所有 `extern "C"` 函数必须 `catch_unwind` 防 panic 跨 FFI
