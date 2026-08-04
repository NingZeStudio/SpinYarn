# SpinYarn 扩展计划：Vanilla & Forge 映射支持

> 状态：**规划中，暂不开发**。原 PLAN（自动下载/快照回归/基准/LRU 缓存/映射外置，5 节）均已实现，本文聚焦下一阶段。
> 动机：LogShare 需覆盖原版/Forge 客户端日志反混淆。当前引擎仅支持 Fabric（Yarn intermediary），原版/Forge 日志直接透传。

---

## 1. 背景与目标

### 现状约束
- 引擎/解析器（`src/mapping/tiny_v2.rs`、`src/deobfuscator/pattern.rs`、`src/deobfuscator/engine.rs`）**全部硬绑定 Yarn intermediary 键**：
  - `normalize_key` 仅剥离 `net/minecraft/` 前缀
  - `Mappings` 仅保留 `class_/method_/field_` 前缀键
  - residual 正则 `class_\d+|method_\d+|field_\d+` + `net[./]minecraft[./]`
- 自动下载（第 1 节已实现）仅定位 Fabric Maven 的 Yarn 映射，版本白名单 `^1\.\d{1,2}(\.\d{1,2})?(-(pre|rc)\d+)?$`

### 目标
- 支持三种映射体系的日志反混淆：
  1. **Yarn**（Fabric，已支持）：键 `class_/method_/field_`
  2. **Vanilla / Mojang official**（原版客户端）：键为单字符短混淆名（`a`/`b`/`fzz`）
  3. **Forge SRG**（Forge 客户端）：键为 `func_<n>_<s>` / `field_<n>_<s>`，类为完整 `net/minecraft/...` 路径
- **版本分发策略与现有 Fabric 统一**：正式版本地缓存（部署携带）、预发布/候选自动下载 + TTL 7 天、快照版跳过（详见 3.3）

---

## 2. 三种映射体系对比

| 维度 | Yarn（Fabric） | Vanilla（Mojang official） | Forge SRG |
|------|---------------|---------------------------|-----------|
| 日志混淆键 | `class_310`/`method_55608`/`field_40713` | 短名 `a`/`b`/`fzz`（类），方法/字段同为短名 | `func_71407_l`/`field_1234_a`，类为完整路径 |
| 键可辨性（正则安全） | 高（`class_\d+` 唯一前缀） | **极低**（单字符，任意文本误匹配） | 高（`func_\d+_`/`field_\d+_` 唯一前缀） |
| 方法重载定位 | 免（`method_XXXX` 全局唯一） | 需**行号区间**（TSRG `start:end`） | 免（`func_` 全局唯一） |
| 映射文件 | tiny v1/v2（gzip） | TSRG（`client.txt`/`server.txt`，纯文本） | SRG（MCP 数据，格式待定） |
| 数据源 | Fabric Maven | Mojang launcher meta | MCP / Forge maven（待核实） |
| 匹配策略 | 结构化 + residual 正则 | **仅结构化堆栈解析**（短名无法正则兜底） | 结构化 + residual 正则（`func_` 安全） |

**核心结论**：
- Vanilla 短名**不能**进 residual 正则兜底（`a.xxx` 会误替换任意文本），必须走 Sherlock 式**结构化堆栈行解析**（类名经映射确认 + 行号定位方法）
- SRG 与 Yarn 均可安全用正则（唯一前缀），可直接并入现有 residual 路径
- Vanilla 需要新增**行号区间索引**（TSRG 方法带 `start:end`），这是与现有「全局唯一键」模型最大的结构性差异

---

## 3. 数据源

### 3.1 Vanilla（Mojang official mappings）—— 已实测
- 定位：`https://launchermeta.mojang.com/mc/game/version_manifest_v2.json` → 版本 `url` → `downloads.client_mappings` / `downloads.server_mappings`
- 实测 1.21.4：`client_mappings` **9.84 MB**、`server_mappings` **7.39 MB**（TSRG 纯文本，未压缩）
- 对比：Yarn `1.21.9.tiny.gz` 仅 1.2MB（gzip），解压文本 ~4.8MB —— Mojang official 文本约为 Yarn 的 **2 倍**（TSRG 含方法行号区间与完整 desc）
- 缓存策略：可复用自动下载的「落盘 + TTL 7 天」机制，存 `<mappings_dir>/vanilla/<version>.txt`

### 3.2 Forge SRG —— 数据源待核实
- 候选：
  - MCP mappings（`mcp.thiakil.com` JSON 接口 / `de.oceanlabs.mcp` maven）
  - Forge 版本安装器内嵌 mapping（`net/minecraftforge:forge` 制品）
  - 需确认：SRG→named 映射的获取途径、格式（CSV/TSRG/其他）、许可
- 预计条目数与大小与 Vanilla 同量级（SRG 是 Mojang obfuscated 与 named 之间的中间层）

### 3.3 自动下载集成

**版本分发策略（三种映射类型统一，与现有 Fabric 一致）**

| 版本类别 | 处理策略 | 说明 |
|---------|---------|------|
| 正式版（如 `1.21.4`） | **本地缓存**（部署携带的 `mappings/` 目录长期驻留，无 TTL） | 由部署/构建流程提供，如现有 43 版本 Yarn 映射 |
| 预发布 / 候选（`1.18.2-pre1`、`1.21.11-rc2`） | **自动下载 + TTL 7 天**（落盘，过期重下载，失败回退旧文件） | 与现有 `ensure_mapping` 行为一致 |
| 快照（`25w44a`、`1.18.2-pre1` 外的周快照） | **跳过**（不下载，直接透传） | `is_downloadable_version` 白名单排除 |
| 26.x（`YY.D.H` / `YY.D Snapshot N`） | **跳过**（无混淆，无需映射，透传即正确） | 沿用现有判断 |

- 实现：现有 `ensure_mapping`/TTL/原子落盘机制扩展为**多数据源**（按 `type` 分目录或分文件，如 `mappings/vanilla/`、`mappings/srg/`），版本白名单（`is_downloadable_version`）对三种类型共用同一套规则（`^1\.\d{1,2}(\.\d{1,2})?(-(pre|rc)\d+)?$`）
- 正式版缓存目录与预发布下载目录可分离：正式版 `mappings/<type>/<version>.<ext>` 部署携带，预发布自动下载至同一目录（带 TTL）
- 数据源按类型切换（Yarn→Fabric Maven、Vanilla→Mojang launcher meta、SRG→数据源待核实），下载/缓存/淘汰逻辑复用

---

## 4. 格式解析

### 4.1 TSRG（Vanilla，Sherlock `VanillaObfuscationMap` 已验证格式）
```
混淆类名 -> 可读类名:
    起始行:结束行:返回类型 混淆方法名(参数desc) -> 可读方法名
    混淆字段名 字段desc -> 可读字段名
```
- 类行：`<obf> -> <named>:`
- 方法行：`<start>:<end>:<returnType> <obfName>(<args>) -> <named>`
- 字段行：`<obfName> <desc> -> <named>`
- 输出：类表（obf→named）、方法表（obf→named）+ **行号区间表**（obfName → Vec<(start,end,named)>）、字段表

### 4.2 SRG（Forge）
- 方法 `func_<编号>_<短码>`、字段 `field_<编号>_<短码>` → named；类为完整路径 `net/minecraft/...` → named
- 格式待数据源核实后确定（CSV / TSRG 变体）

### 4.3 Yarn（现有）
- 保持不变（tiny v1/v2 + `class_/method_/field_` 过滤）

---

## 5. 引擎改造方案

### 5.1 Mappings 结构解耦
- `normalize_key`、键前缀过滤从"硬编码 class_/method_/field_"改为**按映射类型参数化**：
  - Yarn：现有过滤（排除官方短名条目）
  - Vanilla/SRG：保留全部 obf->named 条目（键为短名/SRG 名，无前缀过滤）
- 增加映射类型标记（`MappingType { Yarn, Vanilla, Srg }`）

### 5.2 匹配策略分层
| 路径 | Yarn | Vanilla | SRG |
|------|------|---------|-----|
| 结构化堆栈行（`at <class>.<method>(file:line)`） | 现有 | **主路径**（类确认 + 行号定位） | 主路径 |
| residual 正则兜底 | 现有 | **禁用**（短名误匹配） | 启用（`func_`/`field_` 安全） |

### 5.3 Vanilla 行号定位（新结构）
- `methods_by_line: HashMap<混淆方法名, Vec<(start, end, named)>>`
- 堆栈行 `(file.java:1234)` → 方法名 + 行号 → 命中 `[start,end]` 区间 → 取 named（解决重载）
- 仅 Vanilla 需要；Yarn/SRG 全局唯一键免行号

### 5.4 堆栈行解析改造
- 现有 `map_stack_line` 识别 `class_` 键定位类名。Vanilla 需改为：
  - 提取类路径段 → 查映射（混淆名 或 可读名，双向）→ 确认类存在才处理行
  - 类确认后：方法按映射类型查（Yarn/SRG 全局名，Vanilla 行号）
- 这本质是 Sherlock `ObfuscatedString` 的模型（类必须可映射才重建行），比现有"任一命中即替换"更严谨，需调整现有行为

### 5.5 类型路由
- 请求参数增加 `mapping_type`（`yarn`/`vanilla`/`srg`），默认 `yarn`（向后兼容）
- 或服务端按版本/内容启发推断（Fabric 日志含 `class_`，原版日志类名可读或短名，Forge 含 `func_`）——规划期两种方案并存，推荐**显式参数 + 内容启发兜底**

---

## 6. 缓存与内存

### 6.1 缓存池（共享）
- **单一共享 LRU 缓存池**：Vanilla & Fabric（及未来 SRG）映射共用同一个 `Cache` 实例，key 为「**version + mapping_type**」（同一版本可能同时有 3 类日志，各占一条）
- 默认水位：`max_entries=44` / 高水位 **40** / 低水位 **30**（已调，为共享池预留）

### 6.2 内存估算（实测/推算）
| 项 | 单版本解析表 | 说明 |
|----|------------|------|
| Yarn | ~10 MB（实测） | 现有，1.21.9 |
| Vanilla client | **~12 MB（实测推算）** | 1.21.4 `client.txt` 9.84MB 文本：类 8857 + 方法 76292 + 字段 42508 = **127,657 条目**，三表 ~10.8MB + 行号区间索引 ~1.2MB；键为短混淆名（平均 2.2B）值可读名（平均 20.4B） |
| Forge SRG | **~10 MB** | 与 Vanilla 同源数量，估算 |

### 6.3 峰值内存影响
- **峰值由缓存条目上限决定，与类型数无关**：峰值 ≈ 条目上限 × 平均 ~10-11MB/表
- 默认共享池：低 30 → **~330MB**，峰值 40 → **~440MB**；**到 1GB 需条目上限 ~90+**（当前配置远达不到，且无全量需求）
- 但同版本多类型占多个条目 → 40 条目可能只覆盖 ~13 个版本（每版本 3 类型）→ 热版本覆盖缩水、驱逐更频繁（`/health` 观测）
- 若需 3 类型各 20 版本热缓存 → 条目上限提到 ~60+，内存预算 ~660MB（需评估服务器）

### 6.4 建议
- 立项时先按「同版本多类型」实测真实流量分布，再定条目上限
- 若内存紧张：Vanilla 行号索引可考虑压缩（`(u32,u32)` 紧凑存储）

---

## 7. 测试方案

- **单测**：
  - TSRG 解析（类/方法/字段/行号区间、重载同名方法不同行号）
  - SRG 解析（func_/field_ 键）
  - Vanilla 堆栈行：短名类确认 + 行号定位重载、类未映射整行保留
  - SRG residual 正则（`func_` 不误匹配）
  - 类型路由（`mapping_type` 参数）
- **快照回归扩展**：构造原版/Forge 真实崩溃日志样本（Sherlock 有 Fabric 样本，Vanilla 样本需另备），纳入 `tests/snapshots/`
- **集成**：`mapping_type=vanilla`/`srg` 端到端；自动下载 Vanilla 映射（真实 maven/meta）

---

## 8. 里程碑与工作分解（建议顺序）

1. **M1 数据源**：Vanilla 定位+下载（复用自动下载框架）；Forge SRG 数据源核实
2. **M2 解析器**：TSRG 解析器（含行号区间）；SRG 解析器（格式定后）
3. **M3 引擎**：Mappings 结构解耦 + `MappingType`；Vanilla 结构化堆栈路径（类确认+行号定位）；SRG 并入 residual
4. **M4 路由/缓存**：`mapping_type` 参数 + 内容启发；缓存 key 改 version+type；条目上限重估
5. **M5 测试**：单测 + 快照回归 + 集成 + 内存实测

---

## 9. 风险与待核实项

- 🔴 **Forge SRG 数据源/格式/许可**未核实（MCP 映射的许可约束需确认，可能影响分发方式）
- 🔴 **Vanilla 短名误匹配**是核心难点，结构化解析的正确性需大量真实样本验证
- 🟡 引擎改造面较大（Mappings 结构、堆栈解析行为、类型路由），回归风险需快照测试兜底
- 🟡 内存预算：3 类型热缓存可能需 600MB，需服务器评估
- 🟡 26.1+ 无混淆版本：三类映射均无需下载（沿用现有透传判断）
