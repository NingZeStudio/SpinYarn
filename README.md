# SpinYarn

Rust 编写的 Minecraft 日志反混淆引擎，以两个独立构建产物交付：

1. **`spinyarn`**——Web API 服务（Axum + Tokio）
2. **`libspinyarn_capi`**——C ABI 共享库，供 PHP 等宿主语言嵌入（含 `crates/php/` PHP 8 扩展）

利用 Fabric Yarn 映射表，将混淆堆栈追踪（`class_XXX` / `method_XXX` / `field_XXX`）转换为可读名称。作为 LogShare 的替代反混淆层。

## 特性

- **双产物交付**：`spinyarn`（Web API 服务）+ `libspinyarn_capi`（C ABI 共享库 + PHP 8 扩展），共享同一反混淆核心
- **映射外置部署**：43 个版本（1.14 ~ 1.21.11）的 Yarn 映射放在映射目录（不嵌入二进制），首次运行自动创建并补全
- **自动下载**：请求的 `1.x` 版本不在本地时自动从对应源下载映射（落盘缓存 7 天），新版本/预发布无需改代码
- **全量下载（bootstrap）**：按版本清单逐个检查缺失并补全（Yarn + Vanilla 双家族，无官方映射的版本自动跳过）；Web API 启动时后台自动执行，C ABI/PHP 通过 `spinyarn_bootstrap` 显式调用
- **双映射类型**：支持 Fabric（`yarn`，默认）与 Vanilla（`vanilla`，Mojang official mappings，含行号定位重载）
- **LRU 热门缓存**：有界 LRU（默认 44 条目 + 水位线 40/30，共享缓存池），热版本命中跳过加载（~6ms）；缓存大小可调或禁用（C ABI/PHP 构造参数控制）
- **无缓存模型**：按请求版本加载映射、反混淆、用完即弃，单版本解析表 ~10MB，不随请求版本数增长
- **高性能**：手写 memchr 堆栈解析 + 预编译正则兜底（带 memchr 快速过滤，无键行零成本直通），真实 5MB 日志引擎处理 ~30ms
- **模块前缀处理**：`knot/`、`knot//` 模块前缀、嵌套类、源文件名、描述符、`(Native Method)`/`(Unknown Source)` 全覆盖
- **嵌套类裸键**：`class_7512` 这类缺外层的嵌套键通过反向索引解析为 `DimensionType$MonsterSettings`
- **透传机制**：不支持的版本原样返回，不报错
- **纯文本输出（Web API）**：`/api/v1/deobfuscate/plain` 直接返回 `text/plain` 完整反混淆日志，免 JSON 转义
- **并发限流（Web API）**：`server.max_concurrency`（默认 32）信号量把峰值内存钉在 N×单版本，突发流量 OOM 换成短暂排队

## 快速开始

### 构建（Web API）

```bash
# 编译 Web API
cargo build --release
```

### 构建（C ABI 共享库）

```bash
# 编译 C ABI 库：libspinyarn_capi.so（+ .a 静态库）
cargo build --release -p spinyarn-capi
# 头文件：crates/capi/include/spinyarn.h
```

### PHP 扩展（crates/php/）

PHP 8（NTS）。用 phpize 构建：

```bash
cd crates/php
phpize && ./configure --enable-spinyarn --with-spinyarn-libdir=../../target/release
make
```

无 phpize/autoconf 环境可手动编译：

```bash
cd crates/php
gcc -shared -fPIC -O2 -DCOMPILE_DL_SPINYARN $(php-config --includes) \
  -I ../capi/include spinyarn.c -o spinyarn.so \
  -L ../../target/release -lspinyarn_capi
```

加载与调用（运行时需 `LD_LIBRARY_PATH` 指向 cdylib）：

```php
// MySQLi 风格位置参数：映射目录、自动下载、缓存上限、高水位、低水位
$handle = spinyarn_init(__DIR__ . '/mappings', true);              // 默认缓存 (44/40/30)
$handle = spinyarn_init(__DIR__ . '/mappings', true, 10, 8, 5);    // 自定义缓存水位
$handle = spinyarn_init(__DIR__ . '/mappings', true, 0);           // 禁用 LRU 缓存

// 部署/初始化阶段全量下载默认版本清单（43 Yarn + Vanilla），阻塞调用
$downloaded = spinyarn_bootstrap($handle);

$r = spinyarn_deobfuscate($handle, $log, '1.21.9', SPINYARN_YARN);
// $r = ['deobfuscated' => ..., 'classes_mapped' => 1, 'methods_mapped' => 1, ...]
```

PHP 函数：`spinyarn_init` / `spinyarn_deobfuscate` / `spinyarn_load_mapping` / `spinyarn_has_mapping` / `spinyarn_bootstrap` / `spinyarn_version`；常量 `SPINYARN_YARN` / `SPINYARN_VANILLA`。handle 是 PHP resource，析构自动释放。

### 部署与运行

```bash
# 直接运行即可，无需预先准备映射或配置文件：
./target/release/spinyarn
# 首次启动会自动：
#   1. 在二进制同级生成 config.toml（若不存在）
#   2. 自动创建 mappings/ 并按 bootstrap_versions 清单逐个补全缺失的映射（Yarn + Vanilla）
# 默认监听 127.0.0.1:14523；端口被占用时自动 +1 递增直至找到空闲端口
```

## 文档

完整的对接文档（Web API 接口约定 + C ABI 函数 + PHP 扩展函数）见 **[docs/API.md](docs/API.md)**。维护约定见 `AGENTS.md`。

## 测试

```bash
cargo test --workspace # 单元测试（core/capi/bin 各 crate + 集成 tests/）
bash test.sh           # 集成场景（需先构建 release 二进制）
cargo bench            # 基准测试（引擎吞吐：堆栈/非堆栈/真实/5MB 噪声）
```

真实日志样本：`tests/fixtures/1.21.9-crash.log`、`tests/fixtures/1.21.11-fcl.log.txt`。

## 架构

```
Cargo workspace
├── spinyarn (binary)          # Axum Web API
│     POST /api/v1/deobfuscate        # JSON 响应（deobfuscated + stats）
│     POST /api/v1/deobfuscate/plain  # text/plain 响应
│       → spawn_blocking（受并发限流信号量约束）→ spinyarn_core
├── crates/core                # spinyarn-core（纯 Rust 同步库）
│     Spinyarn 门面：deobfuscate / bootstrap / load_mapping / has_mapping ...
│       → load_mappings(version)  # 外部目录 → 自动下载 → 透传
│       → 解析 v1/v2 → 4 张全局 HashMap（classes/methods/fields/nested）
│       → LineEngine / VanillaEngine
├── crates/capi                # spinyarn-capi（cdylib，C ABI）
└── crates/php                 # PHP 8 扩展（链接 libspinyarn_capi）
```

详见 `AGENTS.md`（维护约定）。

## 许可

本项目使用 **MIT NoFree License**：基于 MIT，但**禁止商用**（非商业、个人、教育、研究用途免费）。详见 [LICENSE](LICENSE)。
