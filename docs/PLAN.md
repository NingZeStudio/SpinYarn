# SpinYarn 扩展计划：Vanilla 映射支持

> 状态：**规划中，暂不开发**。原 PLAN（自动下载/快照回归/基准/LRU 缓存/映射外置，5 节）均已实现，本文聚焦下一阶段。
> 动机：LogShare 需覆盖原版客户端日志反混淆。当前引擎仅支持 Fabric（Yarn intermediary），原版日志直接透传。
> 范围：Vanilla（Mojang official mappings）支持。**Forge SRG 明确不提供**。

---

## 1. 背景与目标

### 现状约束
- 引擎/解析器（`src/mapping/tiny_v2.rs`、`src/deobfuscator/pattern.rs`、`src/deobfuscator/engine.rs`）**全部硬绑定 Yarn intermediary 键**：
  - `normalize_key` 仅剥离 `net/minecraft/` 前缀
  - `Mappings` 仅保留 `class_/method_/field_` 前缀键
  - residual 正则 `class_\d+|method_\d+|field_\d+` + `net[./]minecraft[./]`
- 自动下载（第 1 节已实现）仅定位 Fabric Maven 的 Yarn 映射，版本白名单 `^1\.\d{1,2}(\.\d{1,2})?(-(pre|rc)\d+)?$`

### 目标
- 支持两种映射体系的日志反混淆：
  1. **Yarn**（Fabric，已支持）：键 `class_/method_/field_`
  2. **Vanilla / Mojang official**（原版客户端）：键为单字符短混淆名（`a`/`b`/`fzz`）
- **版本分发策略与现有 Fabric 统一**：正式版本地缓存（部署携带）、预发布/候选自动下载 + TTL 7 天、快照版跳过（详见 3.2）
- **不支持**：Forge SRG（`func_/field_` 键）——明确不在范围

---

## 2. 两种映射体系对比

| 维度 | Yarn（Fabric） | Vanilla（Mojang official） |
|------|---------------|---------------------------|
| 日志混淆键 | `class_310`/`method_55608`/`field_40713` | 短名 `a`/`b`/`fzz`（类），方法/字段同为短名 |
| 键可辨性（正则安全） | 高（`class_\d+` 唯一前缀） | **极低**（单字符，任意文本误匹配） |
| 方法重载定位 | 免（`method_XXXX` 全局唯一） | 需**行号区间**（TSRG `start:end`） |
| 映射文件 | tiny v1/v2（gzip） | TSRG（`client.txt`/`server.txt`，纯文本） |
| 数据源 | Fabric Maven | Mojang launcher meta |
| 匹配策略 | 结构化 + residual 正则 | **仅结构化堆栈解析**（短名无法正则兜底） |

**核心结论**：
- Vanilla 短名**不能**进 residual 正则兜底（`a.xxx` 会误替换任意文本），必须走 Sherlock 式**结构化堆栈行解析**（类名经映射确认 + 行号定位方法）
- Vanilla 需要新增**行号区间索引**（TSRG 方法带 `start:end`），这是与现有「全局唯一键」模型最大的结构性差异

---

## 3. 数据源与版本策略

### 3.1 Vanilla（Mojang official mappings）—— 已实测
- 定位：`https://launchermeta.mojang.com/mc/game/version_manifest_v2.json` → 版本 `url` → `downloads.client_mappings` / `downloads.server_mappings`
- 实测 1.21.4：`client_mappings` **9.84 MB**、`server_mappings` **7.39 MB**（TSRG 纯文本，未压缩）
- 对比：Yarn `1.21.9.tiny.gz` 仅 1.2MB（gzip），解压文本 ~4.8MB —— Mojang official 文本约为 Yarn 的 **2 倍**（TSRG 含方法行号区间与完整 desc）
- 缓存策略：可复用自动下载的「落盘 + TTL 7 天」机制，存 `<mappings_dir>/vanilla/<version>.txt`

### 3.2 版本分发策略（Yarn & Vanilla 统一，与现有 Fabric 一致）

| 版本类别 | 处理策略 | 说明 |
|---------|---------|------|
| 正式版（如 `1.21.4`） | **本地缓存**（部署携带的 `mappings/` 目录长期驻留，无 TTL） | 由部署/构建流程提供，如现有 43 版本 Yarn 映射 |
| 预发布 / 候选（`1.18.2-pre1`、`1.21.11-rc2`） | **自动下载 + TTL 7 天**（落盘，过期重下载，失败回退旧文件） | 与现有 `ensure_mapping` 行为一致 |
| 快照（`25w44a` 周快照） | **跳过**（不下载，直接透传） | `is_downloadable_version` 白名单排除 |
| 26.x（`YY.D.H` / `YY.D Snapshot N`） | **跳过**（无混淆，无需映射，透传即正确） | 沿用现有判断 |

### 3.3 Vanilla 支持版本范围（已实测扫描 version manifest）
- **39 个正式版**（`1.14.4` ~ `1.21.11`）有官方 mappings，与现有 Yarn 的 43 个高度重叠（仅缺 `1.14`~`1.14.3` 四个最早版本）
- `1.0`~`1.14.3`（59 个）无官方映射（Mojang 未提供 TSRG）
- `26.x`（4 个）无混淆
- client.txt 大小随版本增长：1.16.5 = 5.5MB → 1.21.4 = 9.8MB

### 3.4 自动下载集成
- 现有 `ensure_mapping`/TTL/原子落盘机制扩展为**双数据源**（按 `type` 分目录：`mappings/yarn/`、`mappings/vanilla/`），版本白名单共用同一规则（`^1\.\d{1,2}(\.\d{1,2})?(-(pre|rc)\d+)?$`）
- 数据源按类型切换（Yarn→Fabric Maven、Vanilla→Mojang launcher meta），下载/缓存/淘汰逻辑复用

---

## 4. 格式解析

### 4.1 TSRG（Vanilla，Sherlock `VanillaObfuscationMap` 已验证格式）
```
可读类名 -> 混淆类名:
    起始行:结束行:返回类型 可读方法名(参数desc) -> 混淆方法名
    可读字段名 字段desc -> 混淆字段名
```
> 注：TSRG 为 `named -> obf` 方向（左侧可读名、右侧混淆短名）。日志中的混淆短名需**反查**得到可读名。
- 类行：`<named> -> <obf>:`
- 方法行：`<start>:<end>:<returnType> <named>(<args>) -> <obf>`
- 字段行：`<named> <desc> -> <obf>`
- 输出：混淆名→可读名 的三表（类/方法/字段）+ **行号区间表**（混淆方法名 → Vec<(start,end,named)>）

### 4.2 Yarn（现有）
- 保持不变（tiny v1/v2 + `class_/method_/field_` 过滤）

---

## 5. 引擎改造方案

### 5.1 Mappings 结构解耦
- `normalize_key`、键前缀过滤从"硬编码 class_/method_/field_"改为**按映射类型参数化**：
  - Yarn：现有过滤（排除官方短名条目）
  - Vanilla：保留全部混淆名键（短名，无前缀过滤）
- 增加映射类型标记（`MappingType { Yarn, Vanilla }`）

### 5.2 匹配策略分层
| 路径 | Yarn | Vanilla |
|------|------|---------|
| 结构化堆栈行（`at <class>.<method>(file:line)`） | 现有 | **主路径**（类确认 + 行号定位） |
| residual 正则兜底 | 现有 | **禁用**（短名误匹配） |

### 5.3 Vanilla 行号定位（新结构）
- `methods_by_line: HashMap<混淆方法名, Vec<(start, end, named)>>`
- 堆栈行 `(file.java:1234)` → 方法名 + 行号 → 命中 `[start,end]` 区间 → 取 named（解决重载）
- 仅 Vanilla 需要；Yarn 全局唯一键免行号

### 5.4 堆栈行解析改造
- 现有 `map_stack_line` 识别 `class_` 键定位类名。Vanilla 需改为：
  - 提取类路径段 → 查映射（混淆名 或 可读名，双向）→ 确认类存在才处理行
  - 类确认后：方法按映射类型查（Yarn 全局名，Vanilla 行号）
- 这本质是 Sherlock `ObfuscatedString` 的模型（类必须可映射才重建行），比现有"任一命中即替换"更严谨，需调整现有行为

### 5.5 类型路由
- 请求参数增加 `mapping_type`（`yarn`/`vanilla`），默认 `yarn`（向后兼容）
- 或服务端按版本/内容启发推断（Fabric 日志含 `class_`，原版日志类名可读或短名）——规划期两种方案并存，推荐**显式参数 + 内容启发兜底**

---

## 6. 缓存与内存

### 6.1 缓存池（共享）
- **单一共享 LRU 缓存池**：Vanilla & Fabric 映射共用同一个 `Cache` 实例，key 为「**version + mapping_type**」（同一版本可能同时有 2 类日志，各占一条）
- 默认水位：`max_entries=44` / 高水位 **40** / 低水位 **30**（已调，为共享池预留）

### 6.2 内存估算（实测/推算）
| 项 | 单版本解析表 | 说明 |
|----|------------|------|
| Yarn | ~10 MB（实测） | 现有，1.21.9 |
| Vanilla client | **~12 MB（实测推算）** | 1.21.4 `client.txt` 9.84MB 文本：类 8857 + 方法 76292 + 字段 42508 = **127,657 条目**，三表 ~10.8MB + 行号区间索引 ~1.2MB；键为短混淆名（平均 2.2B）值可读名（平均 20.4B） |

### 6.3 峰值内存影响
- **峰值由缓存条目上限决定，与类型数无关**：峰值 ≈ 条目上限 × 平均 ~10-11MB/表
- 默认共享池：低 30 → **~330MB**，峰值 40 → **~440MB**；**到 1GB 需条目上限 ~90+**（当前配置远达不到，且无全量需求）
- Fabric+Vanilla 全量缓存：43 Yarn + 39 Vanilla = 82 表 ≈ **~820MB**（有界 LRU 不会全量驻留）
- 同版本双类型占 2 条目 → 40 条目可能只覆盖 ~20 个版本 → 热版本覆盖缩水、驱逐更频繁（`/health` 观测）

### 6.4 建议
- 立项时先按「同版本双类型」实测真实流量分布，再定条目上限
- 若内存紧张：Vanilla 行号索引可考虑压缩（`(u32,u32)` 紧凑存储）

---

## 7. 测试方案

- **单测**：
  - TSRG 解析（类/方法/字段/行号区间、重载同名方法不同行号、named→obf 反查）
  - Vanilla 堆栈行：短名类确认 + 行号定位重载、类未映射整行保留、短名不误匹配
  - 类型路由（`mapping_type` 参数）
- **快照回归扩展**：构造原版真实崩溃日志样本（Sherlock 有 Fabric 样本，Vanilla 样本需另备），纳入 `tests/snapshots/`
- **集成**：`mapping_type=vanilla` 端到端；自动下载 Vanilla 映射（真实 Mojang meta）

---

## 8. 里程碑与工作分解（建议顺序）

1. **M1 数据源**：Vanilla 定位 + 下载（复用自动下载框架，`mappings/vanilla/`）
2. **M2 解析器**：TSRG 解析器（含行号区间、named→obf 反查）
3. **M3 引擎**：Mappings 结构解耦 + `MappingType`；Vanilla 结构化堆栈路径（类确认 + 行号定位）
4. **M4 路由/缓存**：`mapping_type` 参数 + 内容启发；缓存 key 改 version+type（共享池已就绪）
5. **M5 测试**：单测 + 快照回归 + 集成 + 内存实测

---

## 9. 风险与待核实项

- 🔴 **Vanilla 短名误匹配**是核心难点，结构化解析的正确性需大量真实样本验证
- 🟡 引擎改造面较大（Mappings 结构、堆栈解析行为、类型路由），回归风险需快照测试兜底
- 🟡 内存预算：双类型全量 82 表 ≈ 820MB，有界 LRU 下峰值 40 条目 ≈ 440MB，需服务器评估
- 🟡 26.1+ 无混淆版本：两类映射均无需下载（沿用现有透传判断）
- ✅ 已排除：Forge SRG 明确不在范围（无需核实 MCP/Forge 数据源与许可）
