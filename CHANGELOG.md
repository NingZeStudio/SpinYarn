# 更新日志

本项目版本号跟随 Cargo.toml。所有重要变更均记录于此。

## [Unreleased]

### 变更
- 查找表改用 `ahash::AHashMap`（实测 wall 仅 ~3% 收益，瓶颈在字符串分配而非哈希；零成本保留）
- 反混淆路径加并发限流信号量（`SPINYARN_MAX_CONCURRENCY`，默认 8），峰值内存钉在 N×单版本 ~30MB，防止突发流量 OOM

### 移除
- 响应体 `original` 字段（客户端持有原文，去掉后大日志响应体减半），同时消除 handler 中的 `content.clone()`

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
