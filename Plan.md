# SpinYarn 设计文档

## 概述

Rust 编写的 Minecraft 日志反混淆 Web API。利用 Fabric Yarn 映射将 `class_XXX`/`method_XXX`/`field_XXX` 转为可读名称，替代 LogShare 中的 Aternos Sherlock。

## 核心设计决策

### 1. 无缓存模型
- 每次请求按 `version` 独立加载映射 → 反混淆 → 用完即弃
- 内存恒定单版本 ~30-40MB，不随请求版本数增长
- **不采用 LRU/全量缓存**：LogShare 日志版本来源不可控，缓存命中率趋近 0，淘汰/重载反而徒增 I/O

### 2. 映射表编译期嵌入（单文件部署）
- `build.rs` 扫描 `mappings/*.tiny.gz`，生成 `embedded_mappings.rs`（`include_bytes!`）
- 二进制约 41MB，部署只需单个可执行文件
- 加载优先级：嵌入表 → 外部 `SPINYARN_MAPPINGS_DIR` 覆盖 → 透传
- 构建前提：`mappings/` 目录存在（`scripts/download_mappings.sh`）

### 3. Tiny v1/v2 双格式解析器
- 自动检测头部（`tiny\t2` 或 `v1`）
- 列位置按头部命名空间名定位（兼容 1.14/1.14.1 的 `official named intermediary` 特殊列序）
- 输出 3 张全局 `HashMap`（`class_`/`method_`/`field_` 前缀键，排除官方短名）
- 官方 `-tiny.gz` 实际均为 **Tiny v1 格式**

### 4. LineEngine（高性能反混淆）
- **堆栈行**：手写 memchr 解析 `at <class>.<method>(<file>:<line>)`，支持 `knot/`/`knot//` 前缀、嵌套类回退、源文件名替换
- **非堆栈行**：单个预编译正则兜底，贪婪 `\d+` 免疫 `class_31`/`class_310` 前缀冲突
- 方法/字段键全局唯一（Yarn 编号），免描述符索引
- `String::with_capacity` 预分配 + 单遍构建输出

### 5. 边界与约束
- 支持 43 个版本（1.14 ~ 1.21.11），其余**透传**
- 请求体上限 64MB（Axum 默认 2MB，已调高）
- 管理端点（load/unload/list）在 v2 移除，仅保留 deobfuscate + health

## 性能基线（Termux aarch64，release）

| 场景 | 耗时 |
|------|------|
| 单版本解析 | 1.14.4: 30ms / 1.19.4: 53ms / 1.21.9: 73ms |
| 固定加载成本 | ~100ms |
| 反混淆 5MB 日志 | ~100ms |
| 单请求总耗时（5MB） | ~0.33-0.37s |

## 曾否决的方案

- **AC 自动机**：每请求重建成本 0.3-0.5s，子串匹配需边界检查/排序
- **全量缓存 43 版本**：~1.5GB 内存，远超 Termux 可用量
- **LRU 缓存**：版本来源不可控，命中率趋近 0
