# SpinYarn 项目文档

## 项目概述
Rust 编写的 Minecraft 日志反混淆 Web API 服务，利用 Fabric Yarn 映射表将混淆堆栈追踪（`class_XXX`/`method_XXX`/`field_XXX`）转换为可读名称。作为 LogShare 的替代反混淆层。

## 架构（v2，无缓存模型）
- Axum Web 服务器（默认 `127.0.0.1:8080`）
- **每请求无缓存**：按 version 独立加载映射 → 反混淆 → 用完即弃，内存恒定单版本 ~30MB
- CPU 密集加载/反混淆放入 `tokio::task::spawn_blocking`，不阻塞异步 runtime

### 处理流程（POST /api/v1/deobfuscate）
```
请求(version, content)
  → spawn_blocking 加载映射（嵌入式优先，外部目录可选覆盖）
  → 解析 v1/v2 → 3 张全局 HashMap（classes/methods/fields: intermediary→named）
  → LineEngine：
      堆栈行 → 手写 memchr 解析 + 查表替换
      非堆栈行 → 合并正则兜底（残差）
  → 返回，释放
```

## 关键决策与约定

### 1. 内置 43 个版本 + 透传
`src/config.rs::SUPPORTED_VERSIONS` 硬编码 1.14 ~ 1.21.11 共 43 个版本。反混淆请求：
- 版本在内置列表 → 加载映射正常反混淆
- 否则 → **原样透传**（`success: true`，计数为 0），不报错

### 2. 映射文件嵌入二进制（单文件部署）
- 43 个版本官方 Yarn 映射（gzip）**编译期嵌入二进制**：`build.rs` 扫描 `mappings/<version>.tiny.gz`，生成 `embedded_mappings.rs`（`include_bytes!`），运行时 `embedded::get(version)` 零拷贝取字节
- 二进制约 40MB（9MB 程序 + 31MB 映射数据），**部署只需单个可执行文件**，无需携带 mappings/ 目录
- 加载顺序：嵌入式映射表优先 → 外部 `SPINYARN_MAPPINGS_DIR` 目录覆盖（可选）→ 都没有则透传
- **构建前提**：`mappings/` 目录必须存在（`scripts/download_mappings.sh`）；CI 在 build 前先下载
- 下载脚本：`scripts/download_mappings.sh`（自动探测最新 build）

### 3. Tiny 解析器（v1 + v2 双格式，全局表输出）
`src/mapping/tiny_v2.rs` 自动检测头部（`tiny\t2` 或 `v1`）：
- **v1 平铺**：`CLASS`/`FIELD`/`METHOD`，父类列忽略（全局表不需要）
- **v2 缩进**：`c`/`\tf`/`\tm` 层级
- 列位置按头部命名空间名定位（兼容 1.14/1.14.1 的 `official named intermediary` 特殊列序）
- 输出 3 张全局 `HashMap<String,String>`（键为 `class_`/`method_`/`field_` 前缀，排除官方短名条目）

### 4. LineEngine（高性能反混淆引擎）
`src/deobfuscator/engine.rs`，Sherlock 式结构 + Rust 极致性能：
- **手写堆栈行解析**（memchr）：`at <class>.<method>(<file>:<line>)`，支持模块前缀（`java.base/`、`knot/`）、斜杠/点混合、嵌套类（`class_11980$class_11981` 整体提取，未命中回退外层类 `class_11980`）、`(Native Method)`/`(Unknown Source)`
- **源文件名替换**：`(class_310.java:465)` → `(MinecraftClient.java:465)`（Sherlock 行为）
- **残差正则兜底**（`src/deobfuscator/pattern.rs`）：非堆栈行用单个预编译正则，贪婪 `\d+` 天然免疫 `class_31` vs `class_310` 前缀冲突
- 方法/字段键全局唯一（`method_XXXX`/`field_XXXX`），免描述符索引
- `String::with_capacity` 预分配 + 单遍构建输出，`HashMap::get(&str)` 借用零分配

### 5. API 路由
仅两个端点：
- `POST /api/v1/deobfuscate`（请求体上限 64MB，防超限）
- `GET /api/v1/health`（status + uptime）
管理端点（load/unload/list/version）已在 v2 移除。

## 真实日志验证（1.21.9 FCL + Sodium/Iris 崩溃日志）
`HqZnHhz.log`（69KB，717 行，含 36 处混淆名）反混淆实测：
- 堆栈行类名/方法名/源文件名全部替换：`at knot/net.minecraft.class_310.method_55608(class_310.java:465)` → `at net.minecraft.client.MinecraftClient.method_55608(MinecraftClient.java:465)`
- 嵌套类 `class_11980$class_11981` → `PacketApplyBatcher$Entry`，字段 `this.field_3712.field_1724` → `this.client.player`
- 残留混淆名仅 `class_7512`（该键在 1.21.9 表中不存在，正确保留）
- 总耗时 ~0.26s（含加载），反混淆 ~7ms

## 性能实测（Termux，release）
| 场景 | 数据 |
|------|------|
| 单版本解析（v1，纯 parse，无 gzip） | 1.14.4: 30ms / 1.19.4: 53ms / 1.21.9: 73ms |
| 固定加载成本（gzip + 解析，与日志大小无关） | ~100ms |
| 反混淆 5MB 日志（10 万行） | ~100-125ms |
| 单请求总耗时（69KB 日志，含加载+传输） | ~0.14-0.22s |
| 单请求总耗时（5MB，含加载+传输） | ~0.33-0.37s |
| 43 版本全部反混淆可用 | 43/43 ✓ |
| 峰值内存 | ~30-40MB（单版本） |

解析器优化：去掉每行 `collect::<Vec<&str>>()`（改用栈上小数组）+ HashMap `with_capacity` 预分配，加载成本从 ~150ms 降至 ~100ms。

## 测试
- 单元测试：`cargo test`（15 个：v1/v2 解析、键过滤、LineEngine 堆栈/描述符/透传/前缀冲突/匿名类/嵌套类/源文件名）
- 集成测试：`bash test.sh`（7 个场景：health、v1/v2 真实映射反混淆、描述符、多行日志、透传、错误码）

## 注意事项
- Termux 环境 `/tmp` 只读，测试临时文件用项目目录
- 集成测试若 8080 被占用，先 `pkill -f target/release/spinyarn`
- Axum 默认请求体 2MB，5MB 日志需 64MB limit（已配置，勿删）
- regex crate 不支持 look-around，边界处理用 `\b` + 前缀边界捕获组实现
