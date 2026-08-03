# 更新日志

本项目版本号跟随 Cargo.toml。所有重要变更均记录于此。

## [Unreleased]

### 新增
- 反混淆路径并发限流（`src/api/deobfuscate.rs::GATE`，`SPINYARN_MAX_CONCURRENCY` 可调，默认 32）：无缓存模型下每个在途请求持有 ~30MB 版本表，限流将峰值内存钉在 N×单版本，突发流量 OOM 换成短暂排队
- HTTP 访问日志中间件（`tower_http::TraceLayer`）：记录 method/uri/status/耗时；deobfuscate 走 INFO，health 探针走 DEBUG 降噪
- 默认端口改为 14523，端口被占用时自动 +1 递增直至找到空闲端口
- `POST /api/v1/deobfuscate/plain` 纯文本端点：成功返回 `text/plain` 完整反混淆日志，免 JSON 转义；失败仍返回 JSON 错误
- **嵌套类裸键反向索引**（`Mappings::nested`）：缺外层的裸嵌套键（`class_7512` → `DimensionType$MonsterSettings`）可反混淆，仅对全局唯一内层键建立，重复键跳过防歧义

### 变更
- **breaking change**：`/api/v1/deobfuscate` 响应移除 `original` 字段（客户端持有原文），仅返回 `deobfuscated` + `stats`
- 引擎性能优化：非堆栈行进正则前先 `contains` 快速过滤，无混淆键的行零成本直通，真实 5MB 日志引擎耗时 ~95ms → ~30ms
- **版本动态推断**：删除 `SUPPORTED_VERSIONS` 硬编码清单，运行时按嵌入式表 / 外部映射目录判断，往 `mappings/` 新增版本文件（含 pre-release）自动生效
- **配置统一**：`server.max_body_size`（默认 64MB）/`server.max_concurrency`（默认 32）纳入 `config.toml`，环境变量 `SPINYARN_MAX_CONCURRENCY`/`SPINYARN_MAPPINGS_DIR` 作兜底；并发信号量由静态 `GATE` 改为 `AppState` 注入
- **映射外置**：移除 `build.rs`/`embedded.rs`，映射不再嵌入二进制（二进制 43MB → ~6MB），默认从二进制同级 `./mappings/` 加载；`config.toml` 优先从二进制同级目录查找

### 测试
- 真实日志快照回归（`tests/snapshot_test.rs`）：对 `1.21.9-crash.log`、`1.21.11-fcl.log.txt` 反混淆并与 `tests/snapshots/` 逐字节对比，防引擎行为漂移

### 文档
- `docs/yarn_unmapped_stats.csv`：43 版本 Yarn 未命名键统计（METHOD 34.2% / FIELD 33.2% / CLASS 0.5%），按方法未命名率降序排列

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
