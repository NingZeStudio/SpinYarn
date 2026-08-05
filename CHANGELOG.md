# 更新日志

本项目版本号跟随 Cargo.toml。所有重要变更均记录于此。

## [Unreleased]

### 新增
- **Vanilla（Mojang official mappings）反混淆支持**：
  - TSRG 解析器（`src/mapping/vanilla.rs`：类/方法/字段 + 行号区间，类内方法索引应对短混淆名全局不唯一）
  - Vanilla 结构化堆栈引擎（`src/deobfuscator/vanilla.rs`：类确认 + TSRG 行号区间定位重载，短名不适用 residual 正则，未映射行透传）
  - 调度机（`src/mapping/dispatcher.rs`）：按 `mapping_type`（`yarn`/`vanilla`）分派加载与引擎
  - 自动下载扩展：Vanilla 走 Mojang launcher meta（`mappings/vanilla/<version>.txt`，TTL 7 天），与 Yarn 共用版本白名单
  - 缓存共享池 key 改 `version+mapping_type`，存 `Arc<LoadedMappings>`
  - 实测：`at fda.o(SourceFile.java:14)` → `BeeFlyingSoundInstance.getAlternativeSoundInstance`（1.18.2-pre1 自动下载 6.4MB）

### 变更
- LRU 缓存默认水位调整：`max_entries` 32→44、高水位 30→40、低水位 20→30（共享缓存池，为 Vanilla/Fabric 同池缓存预留；水位 30~40 条目 ≈ 300~400MB）
- 管理端点以映射管理形态回归（`src/api/mappings.rs`）：`/mappings/load`（Maven 拉取/刷新）、`/mappings/load/local`（本地加载，canonicalize 防路径穿越）、`/mappings`（列表）、`/mappings/{type}/{version}`（统计）、`/mappings/{version}`（卸载）；`ApiError` 扩展 `NotFound`/`BadRequest`（404/400）
- **OpenAPI**：`GET /api/v1/openapi.json`（utoipa 4 生成 OpenAPI 3.0，覆盖全部端点）

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
- `docs/PLAN.md`：扩展计划（自动下载 / 快照回归 / 基准 / LRU 缓存 / 映射外置），均已实现

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
