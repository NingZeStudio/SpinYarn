use spinyarn::deobfuscator::VanillaEngine;
use spinyarn::mapping::vanilla::{parse_tsrg, VanillaMappings};

fn load_fixture_tsrg() -> VanillaMappings {
    let content = std::fs::read_to_string("tests/fixtures/test-mappings-vanilla.tsrg").unwrap();
    parse_tsrg(&content).unwrap()
}

fn engine_from_tsrg(s: &str) -> VanillaEngine {
    VanillaEngine::new(parse_tsrg(s).unwrap())
}

/// 反混淆 tests/fixtures/1.21.4-vanilla.log（真实映射裁剪的仿真日志）。
#[test]
fn test_vanilla_realistic_log() {
    let engine = VanillaEngine::new(load_fixture_tsrg());
    let input = std::fs::read_to_string("tests/fixtures/1.21.4-vanilla.log").unwrap();
    let out = engine.deobfuscate(&input);

    // 混淆类+方法，行号定位重载（flk.a 行 771 -> addInitialScreens）
    assert!(
        out.text.contains("at net.minecraft.client.Minecraft.addInitialScreens(SourceFile.java:771)"),
        "got: {}",
        out.text
    );
    // flk.f 行 910 -> run
    assert!(out.text.contains("at net.minecraft.client.Minecraft.run(SourceFile.java:910)"));
    // ard.<init> -> ServerLevel.<init>
    assert!(
        out.text.contains("at net.minecraft.server.level.ServerLevel.<init>(SourceFile.java:5)")
    );
    // fda.b 行 13 -> getTime；fda.<init>
    assert!(out.text.contains("at com.mojang.blaze3d.Blaze3D.getTime(SourceFile.java:13)"));
    assert!(out.text.contains("at com.mojang.blaze3d.Blaze3D.<init>(SourceFile.java:17)"));

    // Caused by 栈内：flk.a 行 730 -> onResourceLoadFinished（同混淆名不同行号）
    assert!(
        out.text.contains("at net.minecraft.client.Minecraft.onResourceLoadFinished(SourceFile.java:730)")
    );
    // flk.c 行 750 -> buildInitialScreens（重载 c：746 -> isGameLoadFinished）
    assert!(
        out.text.contains("at net.minecraft.client.Minecraft.buildInitialScreens(SourceFile.java:750)")
    );

    // java.base/ 模块前缀类未映射 -> 整行保留
    assert!(out.text.contains("at java.base/java.util.concurrent.CompletableFuture$AsyncSupply.run(CompletableFuture.java:1768)"));
    assert!(out.text.contains("at java.base/java.lang.Thread.run(Thread.java:1583)"));

    // 非堆栈行全部透传
    assert!(out.text.contains("[12:34:56] [main/INFO]: Loading Minecraft 1.21.4 with Mojang mappings"));
    assert!(out.text.contains("java.lang.RuntimeException: Failed to tick server"));
    assert!(out.text.contains("Caused by: java.lang.NullPointerException"));
    assert!(out.text.contains("... 5 more"));

    // 统计（classes_mapped 为行数累加：flk×4 + ard + fda×2 = 7）
    assert_eq!(out.classes_mapped, 7);
    assert_eq!(out.methods_mapped, 7);
}

#[test]
fn test_vanilla_line_range_disambiguation() {
    // 同一混淆方法名 `q` 在不同行号区间 -> 不同可读名（注意保持缩进）
    let e = engine_from_tsrg(concat!(
        "com.x.Main -> a:\n",
        "    10:20:void foo() -> q\n",
        "    30:40:void bar() -> q\n",
    ));
    assert_eq!(e.deobfuscate("at a.q(SourceFile.java:12)").text, "at com.x.Main.foo(SourceFile.java:12)");
    assert_eq!(e.deobfuscate("at a.q(SourceFile.java:35)").text, "at com.x.Main.bar(SourceFile.java:35)");
    // 行号落空 -> 取第一个区间
    assert_eq!(e.deobfuscate("at a.q(SourceFile.java:50)").text, "at com.x.Main.foo(SourceFile.java:50)");
}

#[test]
fn test_vanilla_readable_class_confirmed() {
    // 类已是可读名且确认存在 -> 保持；方法混淆名仍替换
    let e = engine_from_tsrg("com.x.Main -> a:\n    10:20:void work() -> b\n");
    let r = e.deobfuscate("at com.x.Main.b(SourceFile.java:15)");
    assert_eq!(r.text, "at com.x.Main.work(SourceFile.java:15)");
    // 可读方法名不再替换（已反混淆，保持原样）
    let r2 = e.deobfuscate("at com.x.Main.work(SourceFile.java:15)");
    assert_eq!(r2.text, "at com.x.Main.work(SourceFile.java:15)");
}

#[test]
fn test_vanilla_unmapped_passthrough() {
    let e = engine_from_tsrg("com.x.Main -> a:\n    10:20:void work() -> b\n");
    // 类未映射 -> 整行保留（不猜）
    let line = "at zz.qq(SourceFile.java:1)";
    assert_eq!(e.deobfuscate(line).text, line);
    // 类映射但方法未映射 -> 类替换，方法保留
    let r = e.deobfuscate("at a.unknown(SourceFile.java:12)");
    assert_eq!(r.text, "at com.x.Main.unknown(SourceFile.java:12)");
    assert_eq!(r.classes_mapped, 1);
    assert_eq!(r.methods_mapped, 0);
}

#[test]
fn test_vanilla_constructor_and_unknown_source() {
    let e = engine_from_tsrg("com.x.Y -> z:\n    0:5:void <init>() -> <init>\n    6:9:void go() -> w\n");
    assert_eq!(e.deobfuscate("at z.<init>(SourceFile.java:3)").text, "at com.x.Y.<init>(SourceFile.java:3)");
    // (Unknown Source) 无行号 -> 方法按首个区间
    assert_eq!(e.deobfuscate("at z.w(Unknown Source)").text, "at com.x.Y.go(Unknown Source)");
    // (Native Method)
    assert_eq!(e.deobfuscate("at z.w(Native Method)").text, "at com.x.Y.go(Native Method)");
}

#[test]
fn test_vanilla_non_stack_lines_untouched() {
    let e = engine_from_tsrg("com.x.Main -> a:\n    0:5:void run() -> b\n");
    let log = "[12:00:00] [main/INFO]: hello\nplain text line\n\tnot a stack line\n";
    assert_eq!(e.deobfuscate(log).text, log);
}
