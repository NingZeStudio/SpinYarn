# SpinYarn

Rust 编写的 Minecraft 日志反混淆 Web API 服务。利用 Fabric Yarn 映射表，将混淆堆栈追踪（`class_XXX` / `method_XXX` / `field_XXX`）转换为可读名称。作为 LogShare 的替代反混淆层。

## 特性

- **单文件部署**：43 个版本（1.14 ~ 1.21.11）的 Yarn 映射**编译期嵌入二进制**（~41MB），部署只需拷贝一个可执行文件
- **无缓存模型**：按请求版本加载映射、反混淆、用完即弃，内存恒定 ~30-40MB，不随请求版本数增长
- **并发限流**：`SPINYARN_MAX_CONCURRENCY`（默认 32）信号量把峰值内存钉在 N×单版本，突发流量 OOM 换成短暂排队
- **高性能**：手写 memchr 堆栈解析 + 预编译正则兜底（带 memchr 快速过滤，无键行零成本直通），真实 5MB 日志引擎处理 ~30ms
- **模块前缀处理**：`knot/`、`knot//` 模块前缀、嵌套类、源文件名、描述符、`(Native Method)`/`(Unknown Source)` 全覆盖
- **嵌套类裸键**：`class_7512` 这类缺外层的嵌套键通过反向索引解析为 `DimensionType$MonsterSettings`
- **透传机制**：不支持的版本原样返回，不报错
- **纯文本输出**：`/api/v1/deobfuscate/plain` 直接返回 `text/plain` 完整反混淆日志，免 JSON 转义

## 快速开始

### 构建

```bash
# 1. 下载映射表（build.rs 嵌入依赖，约 36MB）
bash scripts/download_mappings.sh

# 2. 编译（嵌入 43 个版本映射，链接较慢，约 2-5 分钟）
cargo build --release
```

### 运行

```bash
./target/release/spinyarn
# 默认监听 127.0.0.1:14523；端口被占用时自动 +1 递增直至找到空闲端口
```

配置可通过 `config.toml`（`server.host`/`server.port`/`maven.mappings_dir`）或环境变量 `SPINYARN_MAPPINGS_DIR` 覆盖。

## API

### `POST /api/v1/deobfuscate`

请求体上限 64MB。

```json
{
  "content": "at net.minecraft.class_310.method_55608(Client.java:465)",
  "version": "1.21.9"
}
```

响应：

```json
{
  "success": true,
  "data": {
    "deobfuscated": "at net.minecraft.client.MinecraftClient.method_55608(MinecraftClient.java:465)",
    "stats": {
      "version": "1.21.9",
      "classes_mapped": 1,
      "methods_mapped": 1,
      "fields_mapped": 0,
      "total_time_ms": 0.03
    }
  }
}
```

行为：
- 版本在内置 43 个列表且映射可用 → 正常反混淆
- 否则 → **原样透传**（`success: true`，计数为 0）

### `POST /api/v1/deobfuscate/plain`

请求体与 `/api/v1/deobfuscate` 相同，但成功时直接返回 `text/plain; charset=utf-8` 的完整反混淆日志（免 JSON 转义，大日志更省流量）；失败时仍返回 JSON 错误结构。

```bash
curl -X POST /api/v1/deobfuscate/plain \
  -H 'Content-Type: application/json' \
  -d '{"content": "at net.minecraft.class_310.method_55608(Client.java:465)", "version": "1.21.9"}'
# 响应体（text/plain）：
# at net.minecraft.client.MinecraftClient.method_55608(MinecraftClient.java:465)
```

### `GET /api/v1/health`

```json
{ "success": true, "data": { "status": "healthy", "uptime_seconds": 123 } }
```

## 支持版本

内置 43 个版本：1.14 ~ 1.21.11（完整列表见 `src/config.rs::SUPPORTED_VERSIONS`）。其余版本透传。

## 性能（Termux arm64，release 实测）

| 场景 | 耗时 |
|------|------|
| 单版本解析（gzip + 解析，固定成本） | ~110ms |
| 反混淆真实结构 5MB 日志（引擎，快速过滤后） | ~30ms |
| 反混淆纯混淆 5MB 日志（引擎） | ~100ms |
| 单请求总耗时（5MB，含加载+传输） | ~360-460ms |
| 峰值内存 | ~30-40MB |

## 测试

```bash
cargo test          # 17 个单元测试（解析 + 引擎）
bash test.sh        # 8 个集成场景（需先构建 release 二进制）
```

真实日志样本：`tests/fixtures/1.21.9-crash.log`、`tests/fixtures/1.21.11-fcl.log.txt`。

## 架构

```
POST /api/v1/deobfuscate        # JSON 响应（deobfuscated + stats）
POST /api/v1/deobfuscate/plain  # text/plain 响应（完整反混淆日志）
  → spawn_blocking（受并发限流信号量约束）
  → load_mappings(version)    # 嵌入式表 → 外部目录 → 透传
  → 解析 v1/v2 → 4 张全局 HashMap（classes/methods/fields/nested）
  → LineEngine：
      堆栈行 → 手写 memchr 解析 + 查表替换
      非堆栈行 → 快速过滤 → 合并正则兜底
  → 返回，释放
```

详见 `AGENTS.md`（维护约定）。

## 许可

本项目使用 **MIT NoFree License**：基于 MIT，但**禁止商用**（非商业、个人、教育、研究用途免费）。详见 [LICENSE](LICENSE)。
