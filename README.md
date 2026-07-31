# SpinYarn

Rust 编写的 Minecraft 日志反混淆 Web API 服务。利用 Fabric Yarn 映射表，将混淆堆栈追踪（`class_XXX` / `method_XXX` / `field_XXX`）转换为可读名称。作为 LogShare 的替代反混淆层。

## 特性

- **单文件部署**：43 个版本（1.14 ~ 1.21.11）的 Yarn 映射**编译期嵌入二进制**（~41MB），部署只需拷贝一个可执行文件
- **无缓存模型**：按请求版本加载映射、反混淆、用完即弃，内存恒定 ~30-40MB，不随请求版本数增长
- **高性能**：手写 memchr 堆栈解析 + 预编译正则兜底，反混淆 5MB 日志 ~100ms
- **Sherlock 兼容处理**：`knot/`、`knot//` 模块前缀、嵌套类、源文件名、描述符、`(Native Method)`/`(Unknown Source)` 全覆盖
- **透传机制**：不支持的版本原样返回，不报错

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
# 默认监听 127.0.0.1:8080
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
    "original": "at net.minecraft.class_310.method_55608(Client.java:465)",
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

### `GET /api/v1/health`

```json
{ "success": true, "data": { "status": "healthy", "uptime_seconds": 123 } }
```

## 支持版本

内置 43 个版本：1.14 ~ 1.21.11（完整列表见 `src/config.rs::SUPPORTED_VERSIONS`）。其余版本透传。

## 性能（Termux aarch64，release）

| 场景 | 耗时 |
|------|------|
| 单版本解析（gzip + 解析，固定成本） | ~100ms |
| 反混淆 5MB 日志（10 万行） | ~100ms |
| 单请求总耗时（5MB，含加载+传输） | ~0.33-0.37s |
| 峰值内存 | ~30-40MB |

## 测试

```bash
cargo test          # 15 个单元测试（解析 + 引擎）
bash test.sh        # 7 个集成场景（需先构建 release 二进制）
```

真实日志样本：`tests/fixtures/1.21.9-crash.log`、`tests/fixtures/1.21.11-fcl.log.txt`。

## 架构

```
POST /api/v1/deobfuscate
  → spawn_blocking
  → load_mappings(version)    # 嵌入式表 → 外部目录 → 透传
  → 解析 v1/v2 → 3 张全局 HashMap
  → LineEngine：
      堆栈行 → 手写 memchr 解析 + 查表替换
      非堆栈行 → 合并正则兜底
  → 返回，释放
```

详见 `AGENTS.md`（维护约定）与 `Plan.md`（设计决策）。

## 许可

本项目使用 **MIT NoFree License**：基于 MIT，但**禁止商用**（非商业、个人、教育、研究用途免费）。详见 [LICENSE](LICENSE)。
