# SpinYarn

Rust 编写的 Minecraft 日志反混淆 Web API 服务（Axum + Tokio）。利用 Fabric Yarn 映射表将混淆堆栈追踪（`class_XXX`/`method_XXX`/`field_XXX`）转换为可读名称。

## 架构要点

- **无缓存模型**：每请求按 version 独立加载映射 → 反混淆 → 释放，内存恒定 ~30MB
- **并发限流**：`src/api/mod.rs::AppState` 的 `Semaphore`，默认 32（`config.toml` 的 `server.max_concurrency` 可调，未配置时 `SPINYARN_MAX_CONCURRENCY` 环境变量兜底）。无缓存模型下每并发请求持有一整套版本表（~30MB），限流把峰值内存钉在 N×30MB，突发流量 OOM 换成短暂排队（稳态并发约 16，平时不触发）
- **缓存决策**：LogShare 流量为长尾分布（版本跨度大、无稳定热点），LRU 缓存会被烫成全量缓存（43 版本 × ~30MB ≈ 1.3GB+），已否决。短 TTL 缓存（按 version 缓存 60s + 条目上限）为备选方案，**待收尾实装后按线上表现评估**，在此之前维持无缓存；若未来服务器升级至能承受全量缓存的内存当量，直接上全量缓存亦可
- CPU 密集操作（gzip 解压 + 解析 + 反混淆）放入 `tokio::task::spawn_blocking`，不阻塞 runtime
- **访问日志中间件**：`tower_http::TraceLayer` 记录每个请求的 method/uri/status/耗时。deobfuscate 走 INFO，health 探针走 DEBUG（避免噪音）
- 三个端点：`POST /api/v1/deobfuscate`（64MB 上限）、`POST /api/v1/deobfuscate/plain`（成功返回 `text/plain` 完整日志，失败返回 JSON 错误）、`GET /api/v1/health`
- 管理端点（load/unload/list/version）已在 v2 移除

## 关键约定

### 内置版本 + 透传
**无硬编码版本清单**：`src/mapping/download.rs::is_version_supported` 运行时判断——外部映射目录存在 `<version>.tiny.gz` 即可反混淆，否则 → **原样透传**（`success: true`，计数为 0），不报错。往映射目录新增版本文件（含 pre-release）无需改代码即自动生效。

### 映射外置（与二进制同级部署）
- **映射不嵌入二进制**：`build.rs`/`embedded.rs` 已移除，二进制 ~6MB
- 默认映射目录 = **二进制同级 `./mappings/`**（`std::env::current_exe()` 定位，不依赖工作目录）；`config.toml` 的 `maven.mappings_dir` 或 `SPINYARN_MAPPINGS_DIR` 可覆盖
- **构建前必须运行 `bash scripts/download_mappings.sh`** 下载映射到 `mappings/`；部署时把 `mappings/` 与二进制放同一目录（`test.sh` 会自动拷贝）
- 加载：外部映射目录存在即用，否则透传

### 配置加载
`Config::load()` 按顺序查找：二进制同级 `config.toml` → 当前目录 `config.toml` → `SpinYarn.toml` → `/etc/spinyarn/config.toml`，都没找到则使用默认值（`127.0.0.1:14523`）。配置项：`server.host`/`server.port`/`server.max_body_size`（默认 64MB）/`server.max_concurrency`（默认 32）/`maven.mappings_dir`（默认二进制同级 `./mappings`）；后三项未配置时分别由 `SPINYARN_MAX_CONCURRENCY`/`SPINYARN_MAPPINGS_DIR` 环境变量兜底。启动时若端口已被占用，`main.rs` 自动 `port + 1` 递增重试直至找到空闲端口（`u16` 溢出保护）。

### 版本格式兼容
`src/mapping/tiny_v2.rs` 自动检测 v1（平铺 `CLASS`/`FIELD`/`METHOD`）和 v2（缩进 `c`/`\tf`/`\tm`）格式，列位置按头部命名空间名定位（兼容 1.14 特殊列序）。

### 反混淆引擎
- 堆栈行：手写 memchr 解析（支持 `knot/`、`java.base/` 前缀、嵌套类回退、源文件名替换）
- 非堆栈行：预编译正则兜底（`src/deobfuscator/pattern.rs`），贪婪匹配天然免疫前缀冲突；进正则前先 `contains`（memchr 级）快速过滤无混淆键的行，真实日志大部分行零成本直通
- **嵌套类裸键反向索引**：`Mappings::nested`（`tiny_v2.rs` 构建）把日志中缺外层的裸嵌套键（`class_7512`）解析为完整名（`DimensionType$MonsterSettings`）。仅对全局唯一的内层键建立，150+ 个跨类重复的内层键跳过避免歧义
- 方法/字段键全局唯一（`method_XXXX`/`field_XXXX`），免描述符索引
- **Yarn 数据限制**：约 1/3 方法/字段 named 列即 `method_XXXX` 自身（社区未命名，1.21.9 达 34%），无法反混淆；类命名基本完整（0.5% 未命名）。统计见 `docs/yarn_unmapped_stats.csv`

## 测试

```bash
cargo test              # 单元测试（tests/ 目录，含 fixtures 与快照回归）
bash test.sh            # 集成测试（需先构建 release 二进制）
cargo bench             # 基准测试（benches/deobfuscate.rs，引擎吞吐）
```

- 单元测试：引擎/解析器边界场景 + **真实日志快照回归**（`tests/snapshot_test.rs`：对 `tests/fixtures/` 真实日志反混淆，与 `tests/snapshots/` 逐字节对比，防引擎行为漂移；引擎有意变更输出时删除/重建对应 `.snap` 文件）
- 集成测试依赖 `jq` 和 `curl`。服务端口被占用时自动 +1 递增，不会启动失败。

## CI / Release

- CI（push/PR 到 main/develop）：`cargo check --all-targets` → `cargo test --all-targets` → `cargo build --release` → `bash test.sh`
- Release：commit message 以 `[Build]` 开头时触发多平台构建，Release notes 自动取自 `CHANGELOG.md` 最新版本条目

## 注意事项

- Axum 默认请求体限制 2MB，已配置 64MB 上限（`src/api/mod.rs:20`），不要缩小
- `regex` crate 不支持 look-around，边界处理用 `\b` + 前缀边界捕获组
