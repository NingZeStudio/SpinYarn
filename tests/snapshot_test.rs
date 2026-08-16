use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use spinyarn::deobfuscator::{LineEngine, VanillaEngine};
use spinyarn::mapping::vanilla::parse_tsrg;
use spinyarn::mapping::{parse, Mappings};

/// Load mappings from the external `mappings/<version>.tiny.gz` file
/// (mappings ship alongside the binary / live in the repo root during tests).
/// Returns `None` when the file is absent so snapshot tests can skip cleanly
/// instead of failing on a fresh clone without `scripts/download_mappings.sh`.
fn load_mappings_opt(version: &str) -> Option<Mappings> {
    let gz_path = format!("mappings/{}.tiny.gz", version);
    let mut file = std::fs::File::open(&gz_path).ok()?;
    let mut gz = Vec::new();
    file.read_to_end(&mut gz).ok()?;
    let mut dec = GzDecoder::new(&gz[..]);
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw).ok()?;
    Some(parse(&raw).expect("parse mappings"))
}

fn deobfuscate_fixture(fixture: &str, version: &str) -> String {
    let content = std::fs::read_to_string(fixture)
        .unwrap_or_else(|e| panic!("read {}: {}", fixture, e));
    let engine = LineEngine::new(load_mappings_opt(version).unwrap());
    engine.deobfuscate(&content).text
}

/// Skip the test when the external mapping for `version` is missing.
macro_rules! require_mapping {
    ($version:expr) => {
        if load_mappings_opt($version).is_none() {
            eprintln!(
                "[skip] mappings/{}.tiny.gz not present; \
                 run scripts/download_mappings.sh {}",
                $version, $version
            );
            return;
        }
    };
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
    require_mapping!("1.21.9");
    let out = deobfuscate_fixture("tests/fixtures/1.21.9-crash.log", "1.21.9");
    assert_snapshot("1.21.9-crash.log.snap", &out);
}

#[test]
fn test_snapshot_fcl_1_21_11() {
    require_mapping!("1.21.11");
    let out = deobfuscate_fixture("tests/fixtures/1.21.11-fcl.log.txt", "1.21.11");
    assert_snapshot("1.21.11-fcl.log.txt.snap", &out);
}

#[test]
fn test_snapshot_vanilla_1_21_4() {
    // Vanilla: self-contained fixture (real 1.21.4 mapping slice + simulated log).
    let tsrg = std::fs::read_to_string("tests/fixtures/test-mappings-vanilla.tsrg").unwrap();
    let mappings = parse_tsrg(&tsrg).unwrap();
    let engine = VanillaEngine::new(mappings);
    let input = std::fs::read_to_string("tests/fixtures/1.21.4-vanilla.log").unwrap();
    let out = engine.deobfuscate(&input);
    assert_snapshot("1.21.4-vanilla.log.snap", &out.text);
}

#[test]
fn test_snapshot_sherlock_1_18_2_pre1() {
    // Sherlock 测试样本（Fabric，pre-release）。
    require_mapping!("1.18.2-pre1");
    let out = deobfuscate_fixture("tests/fixtures/1.18.2-pre1.log", "1.18.2-pre1");
    assert_snapshot("1.18.2-pre1.log.snap", &out);
}

#[test]
fn test_snapshot_sherlock_1_21_3() {
    // Sherlock 测试样本（Fabric crash report）。
    require_mapping!("1.21.3");
    let out = deobfuscate_fixture("tests/fixtures/1.21.3-crash-report.txt", "1.21.3");
    assert_snapshot("1.21.3-crash-report.txt.snap", &out);
}
