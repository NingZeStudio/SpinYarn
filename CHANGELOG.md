# 更新日志

本项目版本号跟随 Cargo.toml。所有重要变更均记录于此。

## [v0.3.3] - 2026-08-16

### 新增
- **映射外置部署**：映射表不再提交进 git（`build.rs`/`embedded.rs` 已移除，二进制 ~6MB），`mappings/` 由 `.gitignore` 忽略，部署时与二进制同级放置
- **启动引导自动下载**：`maven.auto_download` 开启且 `mappings/` 目录为空时，`main.rs::bootstrap_mappings` 后台任务自动下载 `maven.bootstrap_versions` 清单（默认 1.14~1.21.11 共 43 个版本的 Yarn + Vanilla 双家族映射，可配置），不阻塞启动、不占请求路径；无官方映射的版本自动跳过
- **接入文档**：`docs/API.md`（部署、接口约定、错误码、各端点示例、Python/JS 客户端接入）

### 变更
- `scripts/download_mappings.sh` 支持命令行传版本参数（只下指定版本），重构出 `process_version` 返回码
- 复用进程级 `HTTP_AGENT` 连接池；Vanilla manifest 进程内缓存（TTL 10 分钟），43 版本引导从 43 次拉取降为 1 次
- `dispatcher::load` 改用结构化错误 `LoadError`（Yarn/Vanilla 变体），取代 `String`

### 修复（Code Review 全量修复）
- **路径穿越漏洞（严重）**：`POST /mappings/load/local` 与 `DELETE /mappings/{version}` 未校验 `version` 令牌，可构造 `../` 逃逸 mappings 目录任意文件写入/删除 → `is_valid_version` 提升 `pub(crate)`，新增 `validate_version` 接入三个管理端点，`dispatcher::remove_local` 与 Vanilla 加载路径二次防御
- **500 错误详情脱敏**：`ApiError::Internal` 详情仅 `tracing::error!` 记录，响应返回通用文案
- **OpenAPI 版本号漂移**：运行时以 `env!("CARGO_PKG_VERSION")` 覆盖 `info.version`，根治硬编码
- **并发下载 `.tmp` 竞争**：`unique_tmp` 纳秒时间戳后缀避免同版本并发覆盖
- **`safe_local_path` 过度拦截**：移除 `contains("..")` 预检，仅保留 canonicalize 权威校验（兼顾合法 `foo..bar` 文件名）
- **bench 硬依赖映射**：映射缺失时优雅跳过，不再 panic

### 测试
- 单测 27 → 31（新增 `validate_version`/`safe_local_path`/`remove_local` 穿越与合法路径回归）
- 快照测试映射缺失时静默跳过（`require_mapping!`），全新 clone 可直接 `cargo test`
- CI 新增测试映射下载步骤（只下快照/集成所需 6 版本）；`test.sh` 缺失映射立即报错提示下载命令

### 文档
- `CRCLASH.md` 全面 Code Review 报告（含修复记录）
- `AGENTS.md`/`README.md` 同步启动引导下载与配置项说明

## [v0.3.2] - 2026-08-05

### 新增
- **Vanilla（Mojang official mappings）反混淆支持**：
  - TSRG 解析器（`src/mapping/vanilla.rs`：类/方法/字段 + 行号区间，类内方法索引应对短混淆名全局不唯一）
  - Vanilla 结构化堆栈引擎（`src/deobfuscator/vanilla.rs`：类确认 + TSRG 行号区间定位重载，短名不适用 residual 正则，未映射行透传）
  - 调度机（`src/mapping/dispatcher.rs`）：按 `mapping_type`（`yarn`/`vanilla`）分派加载与引擎
  - 自动下载扩展：Vanilla 走 Mojang launcher meta（`mappings/vanilla/<version>.txt`，TTL 7 天），与 Yarn 共用版本白名单
  - 缓存共享池 key 改 `version+mapping_type`，存 `Arc<LoadedMappings>`
  - 实测：`at fda.o(SourceFile.java:14)` → `BeeFlyingSoundInstance.getAlternativeSoundInstance`（1.18.2-pre1 自动下载 6.4MB）
- **映射管理端点**（`src/api/mappings.rs`）：`POST /mappings/load`（Maven 拉取/刷新，原子覆盖）、`POST /mappings/load/local`（本地加载，canonicalize 防路径穿越）、`GET /mappings`（列表）、`GET /mappings/{type}/{version}`（统计）、`DELETE /mappings/{version}`（卸载文件+缓存）
- **OpenAPI**：`GET /api/v1/openapi.json`（utoipa 4 生成 OpenAPI 3.0，覆盖全部端点）
- `scripts/download_vanilla_mappings.py`：批量下载 39 个正式版 Vanilla 映射到 `mappings/vanilla/`（增量）

### 变更
- LRU 缓存默认水位调整：`max_entries` 32→44、高水位 30→40、低水位 20→30（共享缓存池，为 Vanilla/Fabric 同池缓存预留；水位 30~40 条目 ≈ 300~400MB）
- `ApiError` 扩展 `NotFound`/`BadRequest`（404/400）；`Cache` 增 `remove`
- 管理端点以映射管理形态回归（v0.2 曾移除）

### 修复
- `/mappings/load` refresh 先删后下、失败丢旧文件 → 改为 `ensure_*` 加 `force` 参数（跳过 TTL 原子覆盖，失败保留旧文件）

### 测试
- Vanilla 仿真日志测试（`tests/test_vanilla.rs`：真实 1.21.4 映射裁剪，行号重载/构造器/java.base/透传）与快照回归
- Sherlock 样本快照回归（`1.21.3` 与参考 `.mapped.txt` 逐行一致、`1.18.2-pre1`）
- 单测 26 → 52

### 文档
- `CRCLASH.md` 全面 Code Review 报告
- `docs/PLAN.md` 已完结移除（扩展计划全部实现）

## [v0.3.1] - 2026-08-04

### 新增
- 反混淆路径并发限流（`src/api/mod.rs::AppState`，`SPINYARN_MAX_CONCURRENCY` 可调，默认 32）：无缓存模型下每个在途请求持有 ~10MB 版本表，限流将峰值内存钉在 N×单版本，突发流量 OOM 换成短暂排队
- HTTP 访问日志中间件（`tower_http::TraceLayer`）：记录 method/uri/status/耗时；deobfuscate 走 INFO，health 探针走 DEBUG 降噪
- 默认端口改为 14523，端口被占用时自动 +1 递增直至找到空闲端口
- `POST /api/v1/deobfuscate/plain` 纯文本端点：成功返回 `text/plain` 完整反混淆日志，免 JSON 转义；失败仍返回 JSON 错误
- **嵌套类裸键反向索引**（`Mappings::nested`）：缺外层的裸嵌套键（`class_7512` → `DimensionType$MonsterSettings`）可反混淆，仅对全局唯一内层键建立，重复键跳过防歧义
- **运行时自动下载**（`maven.auto_download`，默认 true）：`1.x` 系缺失版本（含 `-pre`/`-rc`，排除 `25wxx` 快照与 26.x）自动从 Fabric Maven 下载映射落盘，TTL 7 天，过期重下载失败回退旧文件
- **有界 LRU 缓存**（`[cache]` 段，`src/cache.rs`）：解析后表以 `Arc<Mappings>` 缓存，默认上限 32 条目、高水位 30 触发批量淘汰至低水位 20；命中跳过加载（热请求 ~6ms）且不占并发信号量；命中/驱逐/条目数经 `/health` 暴露。实测：单版本解析表 ~10MB，缓存 20 条目 ~189MB、峰值 30 条目 ~300MB
- **缓存/自动下载操作日志**：`cache insert/evict`（INFO，含版本与条目数）、`cache hit/miss`（DEBUG）、`mapping download/downloaded`（INFO）、下载失败回退（WARN）

### 变更
- **breaking change**：`/api/v1/deobfuscate` 响应移除 `original` 字段（客户端持有原文），仅返回 `deobfuscated` + `stats`
- 引擎性能优化：非堆栈行进正则前先 `contains` 快速过滤，无混淆键的行零成本直通，真实 5MB 日志引擎耗时 ~95ms → ~30ms
- **版本动态推断**：删除 `SUPPORTED_VERSIONS` 硬编码清单，运行时按外部映射目录判断，往 `mappings/` 新增版本文件（含 pre-release）自动生效
- **配置统一**：`server.max_body_size`（默认 64MB）/`server.max_concurrency`（默认 32）纳入 `config.toml`，环境变量 `SPINYARN_MAX_CONCURRENCY`/`SPINYARN_MAPPINGS_DIR` 作兜底；并发信号量由静态 `GATE` 改为 `AppState` 注入
- **映射外置**：移除 `build.rs`/`embedded.rs`，映射不再嵌入二进制（二进制 43MB → ~6MB），默认从二进制同级 `./mappings/` 加载；`config.toml` 优先从二进制同级目录查找
- 堆栈行类名未映射但方法命中时剥离模块前缀（`knot//`/`knot/`），输出更可读
- LRU recency 改用进程级原子 tick（`AtomicU64`），`get` 恒更新访问序，并发下唯一递增

### 安全
- 版本参数路径遍历防护：`version` 白名单校验（字母数字开头 + 仅 `.-_` 字符 + 禁 `..`），防止经映射目录路径逃逸读取任意文件

### 测试
- 真实日志快照回归（`tests/snapshot_test.rs`）：对 `1.21.9-crash.log`、`1.21.11-fcl.log.txt` 反混淆并与 `tests/snapshots/` 逐字节对比，防引擎行为漂移
- 基准测试（`benches/deobfuscate.rs`，criterion）：纯堆栈/纯非堆栈/真实日志/5MB 噪声吞吐，实测真实日志 ~350µs、5MB ~23ms（快速过滤生效）
- 缓存单测（命中/水位线淘汰/原子 tick 并发安全/`get` 更新 recency）、版本白名单单测

### 文档
- `docs/yarn_unmapped_stats.csv`：43 版本 Yarn 未命名键统计（METHOD 34.2% / FIELD 33.2% / CLASS 0.5%），按方法未命名率降序排列
- 扩展计划（自动下载 / 快照回归 / 基准 / LRU 缓存 / 映射外置）均已实现，`docs/PLAN.md` 已完结移除

## [v0.3.0] - 2026-07-31

### 新增
- 映射表编译期嵌入二进制（`build.rs` + `include_bytes!`），实现单文件部署（~41MB）
- 加载优先级：嵌入式映射 → 外部 `SPINYARN_MAPPINGS_DIR` 覆盖 → 透传

### 变更
- 移除 Maven 下载回退（删除 `reqwest`/`zip` 依赖）
- 精简 `error.rs` 仅保留 `Internal`
- 配置项收敛为 `server` + `maven.mappings_dir`

### 移除
- `scripts/preload.sh`（依赖已删除的 `/mappings/load` 端点）
- 遗留调试测试 `debug_load.rs`

## [v0.2.0] - 2026-07-31

### 新增
- 无缓存模型：按请求加载映射、用完即弃
- 全局三表（classes/methods/fields）替换三元组键
- `LineEngine`：手写堆栈解析 + 合并正则兜底
- 源文件名替换（`class_310.java` → `MinecraftClient.java`）
- 解析器去分配优化（栈上小数组 + HashMap 预分配）

### 变更
- Tiny 解析器支持 v1+v2 双格式，按头部命名空间定位列
- 移除全部管理端点（load/unload/list/version），仅留 deobfuscate + health
- 请求体上限调至 64MB

## [v0.1.0] - 2026-07

### 初始版本
- Axum Web API + AC 自动机反混淆引擎
- Tiny v2 解析器 + 三元组 MappingTable
- 43 版本映射内置（`mappings/` 目录）
