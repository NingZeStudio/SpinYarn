use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use spinyarn::deobfuscator::LineEngine;
use spinyarn::mapping::{parse, Mappings};

/// Load mappings from the external `mappings/<version>.tiny.gz` file
/// (mappings ship alongside the binary / live in the repo root during tests).
fn load_mappings(version: &str) -> Mappings {
    let gz_path = format!("mappings/{}.tiny.gz", version);
    let mut file = std::fs::File::open(&gz_path)
        .unwrap_or_else(|e| panic!("open {}: {}", gz_path, e));
    let mut gz = Vec::new();
    file.read_to_end(&mut gz).unwrap();
    let mut dec = GzDecoder::new(&gz[..]);
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw).unwrap();
    parse(&raw).expect("parse mappings")
}

fn deobfuscate_fixture(fixture: &str, version: &str) -> String {
    let content = std::fs::read_to_string(fixture)
        .unwrap_or_else(|e| panic!("read {}: {}", fixture, e));
    let engine = LineEngine::new(load_mappings(version));
    engine.deobfuscate(&content).text
}

/// Compare against a stored snapshot; generate it on first run. Prevents the
/// engine's output from silently drifting on real logs.
fn assert_snapshot(name: &str, actual: &str) {
    let dir = "tests/snapshots";
    let path = format!("{}/{}", dir, name);
    if !Path::new(&path).exists() {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, actual).unwrap();
        eprintln!("[snapshot] generated {}", path);
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(expected, actual, "snapshot mismatch for {}", path);
}

#[test]
fn test_snapshot_crash_1_21_9() {
    let out = deobfuscate_fixture("tests/fixtures/1.21.9-crash.log", "1.21.9");
    assert_snapshot("1.21.9-crash.log.snap", &out);
}

#[test]
fn test_snapshot_fcl_1_21_11() {
    let out = deobfuscate_fixture("tests/fixtures/1.21.11-fcl.log.txt", "1.21.11");
    assert_snapshot("1.21.11-fcl.log.txt.snap", &out);
}
