use std::hint::black_box;
use std::io::Read;

use criterion::{criterion_group, criterion_main, Criterion};
use flate2::read::GzDecoder;
use spinyarn::deobfuscator::LineEngine;
use spinyarn::mapping::{parse, Mappings};

/// Load mappings from the external `mappings/<version>.tiny.gz` file once;
/// the bench loops only exercise `deobfuscate`, not mapping loading.
/// Returns `None` when the file is absent so the bench can exit gracefully on
/// a fresh clone instead of panicking (mappings are not committed).
fn load_mappings(version: &str) -> Option<Mappings> {
    let mut file = std::fs::File::open(format!("mappings/{}.tiny.gz", version)).ok()?;
    let mut gz = Vec::new();
    file.read_to_end(&mut gz).ok()?;
    let mut dec = GzDecoder::new(&gz[..]);
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw).ok()?;
    Some(parse(&raw).expect("parse mappings"))
}

fn bench_deobfuscate(c: &mut Criterion) {
    let Some(mappings) = load_mappings("1.21.9") else {
        eprintln!(
            "[skip] mappings/1.21.9.tiny.gz not present; \
             run scripts/download_mappings.sh 1.21.9 to enable benches"
        );
        return;
    };
    let engine = LineEngine::new(mappings);

    // Pure stack lines: class + method + source-file remap + module prefix strip.
    let stack_line = "\tat knot//net.minecraft.class_310.method_21613(class_310.java:465) ~[client-intermediary.jar:?]\n";
    let stack_log = stack_line.repeat(5000);

    // Pure non-stack lines carrying obfuscated keys (regex residual path).
    let nonstack_line = "net.minecraft.class_4355: Realms authentication error with message RuntimeException Failed parse\n";
    let nonstack_log = nonstack_line.repeat(5000);

    // Real crash log.
    let real_log = std::fs::read_to_string("tests/fixtures/1.21.9-crash.log").unwrap();

    // ~5MB real-structure log: mostly noise lines, exercises the fast filter.
    let target = 5 * 1024 * 1024;
    let reps = target / real_log.len() + 1;
    let mut big_log = real_log.clone().repeat(reps);
    // Truncate on a char boundary to avoid panicking on non-ASCII content.
    big_log.truncate(big_log.floor_char_boundary(target));

    c.bench_function("deobfuscate/pure_stack_5k", |b| {
        b.iter(|| black_box(engine.deobfuscate(black_box(&stack_log))))
    });
    c.bench_function("deobfuscate/pure_nonstack_5k", |b| {
        b.iter(|| black_box(engine.deobfuscate(black_box(&nonstack_log))))
    });
    c.bench_function("deobfuscate/real_1_21_9", |b| {
        b.iter(|| black_box(engine.deobfuscate(black_box(&real_log))))
    });
    c.bench_function("deobfuscate/noise_5mb", |b| {
        b.iter(|| black_box(engine.deobfuscate(black_box(&big_log))))
    });
}

criterion_group!(benches, bench_deobfuscate);
criterion_main!(benches);
