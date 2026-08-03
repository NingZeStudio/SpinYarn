use spinyarn::deobfuscator::LineEngine;
use spinyarn::mapping::parse;

fn engine_from_fixture(path: &str) -> LineEngine {
    let input = std::fs::read(path).unwrap();
    let m = parse(&input).unwrap();
    LineEngine::new(m)
}

#[test]
fn test_stack_line_class_and_method() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let r = engine.deobfuscate("at net.minecraft.class_1799.method_1234(ItemStack.java:42)");
    assert!(r.text.contains("net.minecraft.item.ItemStack.getCount"));
    assert_eq!(r.classes_mapped, 1);
    assert_eq!(r.methods_mapped, 1);
}

#[test]
fn test_stack_line_slash_prefix() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let r = engine.deobfuscate("at net/minecraft/class_1799.method_1234(ItemStack.java:42)");
    assert!(r.text.contains("net.minecraft.item.ItemStack.getCount"));
}

#[test]
fn test_descriptor_residual() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let r = engine.deobfuscate("method_1234(Lnet/minecraft/class_1799;)V");
    assert!(r.text.contains("Lnet/minecraft/item/ItemStack;"));
}

#[test]
fn test_bare_class_in_paren() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let r = engine.deobfuscate("at net.minecraft.class_1799.method_1234(class_1799.java:42)");
    assert!(r.text.contains("net.minecraft.item.ItemStack"));
}

#[test]
fn test_unknown_source_and_native_method() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let r = engine.deobfuscate(
        "at net.minecraft.class_1297.method_6004(Unknown Source)\nat java.base/java.lang.Thread.run(Native Method)",
    );
    assert!(r.text.contains("net.minecraft.entity.Entity.tick(Unknown Source)"));
    // Module prefix untouched, `run` not a method_ key -> unchanged.
    assert!(r.text.contains("java.base/java.lang.Thread.run(Native Method)"));
}

#[test]
fn test_anonymous_class_falls_back_to_outer() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let r = engine.deobfuscate("at net.minecraft.class_1297$1.run(Entity.java:10)");
    // class_1297$1 not in table -> falls back to class_1297 -> Entity
    assert!(r.text.contains("net.minecraft.entity.Entity"));
}

#[test]
fn test_nested_class_full_key() {
    // class_11980$class_11981 must be captured whole, not truncated at inner class_.
    let input = b"v1\tofficial\tintermediary\tnamed\n\
                  CLASS\txb\tnet/minecraft/class_11980\tnet/minecraft/network/PacketApplyBatcher\n\
                  CLASS\txb$a\tnet/minecraft/class_11980$class_11981\tnet/minecraft/network/PacketApplyBatcher$Entry\n\
                  METHOD\txb\t()V\tapply\tmethod_74450\tapply\n";
    let m = parse(input).unwrap();
    let engine = LineEngine::new(m);
    let r = engine.deobfuscate("at knot/net.minecraft.class_11980$class_11981.method_74450(class_11980.java:55)");
    assert!(
        r.text.contains("net.minecraft.network.PacketApplyBatcher$Entry.apply"),
        "got: {}",
        r.text
    );
    // source file name remapped too
    assert!(r.text.contains("(PacketApplyBatcher.java:55)"), "got: {}", r.text);
}

#[test]
fn test_stack_line_module_prefix_readable_class() {
    // Class name already readable (not in class_ table) but method maps:
    // the `knot//` module prefix must be stripped, matching Sherlock.
    let input = b"v1\tofficial\tintermediary\tnamed\n\
                  CLASS\ta\tnet/minecraft/class_310\tnet/minecraft/client/MinecraftClient\n\
                  METHOD\ta\t()V\trun\tmethod_1806\trun\n";
    let m = parse(input).unwrap();
    let engine = LineEngine::new(m);
    let r = engine.deobfuscate("at knot//net.minecraft.server.MinecraftServer.method_1806(MinecraftServer.java:1755)");
    assert!(
        r.text.contains("at net.minecraft.server.MinecraftServer.run(MinecraftServer.java:1755)"),
        "got: {}",
        r.text
    );
    // dotted module prefixes (java.base/) must stay intact on unmapped lines
    let r2 = engine.deobfuscate("at java.base/java.lang.Thread.run(Thread.java:1583)");
    assert_eq!(r2.text, "at java.base/java.lang.Thread.run(Thread.java:1583)");
}

#[test]
fn test_non_stack_line_passthrough() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    let log = "[00:00:01] [main/INFO]: Starting Minecraft\nERROR: something went wrong\n";
    let r = engine.deobfuscate(log);
    assert_eq!(r.text, log);
}

#[test]
fn test_prefix_conflict_class_31_vs_310() {
    // class_31 must NOT match inside class_310.
    let input = b"v1\tofficial\tintermediary\tnamed\n\
                  CLASS\ta\tnet/minecraft/class_31\tnet/minecraft/ThirtyOne\n\
                  CLASS\tb\tnet/minecraft/class_310\tnet/minecraft/ThreeHundredTen\n";
    let m = parse(input).unwrap();
    let engine = LineEngine::new(m);

    let r = engine.deobfuscate("at net.minecraft.class_310.method_1234(X.java:1)");
    assert!(r.text.contains("net.minecraft.ThreeHundredTen"), "got: {}", r.text);
    assert!(!r.text.contains("ThirtyOne"), "got: {}", r.text);
}

#[test]
fn test_empty_input() {
    let engine = engine_from_fixture("tests/fixtures/test-mappings-v1.tiny");
    assert_eq!(engine.deobfuscate("").text, "");
}

#[test]
fn test_bare_nested_class_reverse_lookup() {
    // A log line referencing only the inner key of a nested class
    // (`class_7512` for `class_2874$class_7512`) must still resolve via the
    // reverse `nested` table when the inner key is globally unique.
    let input = b"v1\tofficial\tintermediary\tnamed\n\
                  CLASS\ta\tnet/minecraft/class_2874\tnet/minecraft/world/dimension/DimensionType\n\
                  CLASS\ta$b\tnet/minecraft/class_2874$class_7512\tnet/minecraft/world/dimension/DimensionType$MonsterSettings\n";
    let m = parse(input).unwrap();
    let engine = LineEngine::new(m);

    let r = engine.deobfuscate("monsterSettings=class_7512[piglinSafe=false]");
    assert!(
        r.text.contains("DimensionType$MonsterSettings"),
        "got: {}",
        r.text
    );
}

#[test]
fn test_bare_nested_class_ambiguous_skipped() {
    // When an inner key appears under multiple outer classes it is ambiguous
    // and must NOT populate the reverse table.
    let input = b"v1\tofficial\tintermediary\tnamed\n\
                  CLASS\ta\tnet/minecraft/class_2874$class_7512\tnet/minecraft/A$MonsterSettings\n\
                  CLASS\tb\tnet/minecraft/class_9999$class_7512\tnet/minecraft/B$Other\n";
    let m = parse(input).unwrap();
    let engine = LineEngine::new(m);

    let r = engine.deobfuscate("monsterSettings=class_7512[piglinSafe=false]");
    assert!(!r.text.contains("MonsterSettings"), "got: {}", r.text);
}
