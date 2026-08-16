use std::sync::Arc;

use crate::deobfuscator::{DeobfuscateResult, LineEngine, VanillaEngine};
use crate::mapping::download::{is_valid_version, is_version_supported, load_mappings};
use crate::mapping::vanilla::{is_vanilla_supported, load_vanilla_mappings, VanillaMappings};
use crate::mapping::Mappings;

/// The mapping family a request is deobfuscated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingType {
    Yarn,
    Vanilla,
}

impl MappingType {
    pub fn parse(s: &str) -> MappingType {
        match s {
            "vanilla" => MappingType::Vanilla,
            _ => MappingType::Yarn,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MappingType::Yarn => "yarn",
            MappingType::Vanilla => "vanilla",
        }
    }

    /// Cache key segment; `version + mapping_type` keeps the two families
    /// distinct in the shared LRU pool.
    pub fn cache_key(self, version: &str) -> String {
        format!("{}:{}", self.as_str(), version)
    }
}

/// A loaded mapping set (Arc-shared), dispatched to the matching engine.
pub enum LoadedMappings {
    Yarn(Arc<Mappings>),
    Vanilla(Arc<VanillaMappings>),
}

/// Standard on-disk location of a version's mapping file.
pub fn local_path(version: &str, mappings_dir: &str, mtype: MappingType) -> std::path::PathBuf {
    let base = std::path::Path::new(mappings_dir);
    match mtype {
        MappingType::Yarn => base.join(format!("{}.tiny.gz", version)),
        MappingType::Vanilla => base.join("vanilla").join(format!("{}.txt", version)),
    }
}

/// Remove the local mapping file for a version/type (used for refresh/unload).
/// Refuses invalid version tokens (path traversal) regardless of caller.
pub fn remove_local(version: &str, mappings_dir: &str, mtype: MappingType) -> bool {
    if !is_valid_version(version) {
        tracing::warn!("refusing to remove mapping with invalid version: {:?}", version);
        return false;
    }
    let path = local_path(version, mappings_dir, mtype);
    match std::fs::remove_file(&path) {
        Ok(_) => {
            tracing::info!("mapping removed: {} {}", mtype.as_str(), version);
            true
        }
        Err(_) => false,
    }
}

/// Remove local files for both mapping families of a version; returns removed paths.
pub fn remove_all_local(version: &str, mappings_dir: &str) -> Vec<String> {
    let mut removed = Vec::new();
    for mtype in [MappingType::Yarn, MappingType::Vanilla] {
        if remove_local(version, mappings_dir, mtype) {
            removed.push(local_path(version, mappings_dir, mtype).to_string_lossy().into_owned());
        }
    }
    removed
}

/// Whether a mapping for `version` is available locally for the given type.
pub fn is_supported(version: &str, mappings_dir: &str, mtype: MappingType) -> bool {
    match mtype {
        MappingType::Yarn => is_version_supported(version, mappings_dir),
        MappingType::Vanilla => is_vanilla_supported(version, mappings_dir),
    }
}

/// Unified load error for both mapping families (kept structured instead of a
/// `String` so callers/logs can distinguish the source).
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("yarn mapping load: {0}")]
    Yarn(#[from] crate::mapping::download::MappingLoadError),
    #[error("vanilla mapping load: {0}")]
    Vanilla(#[from] crate::mapping::vanilla::VanillaParseError),
}

/// Load mappings for `version` using the given mapping type.
pub fn load(
    version: &str,
    mappings_dir: &str,
    mtype: MappingType,
) -> Result<Option<LoadedMappings>, LoadError> {
    match mtype {
        MappingType::Yarn => load_mappings(version, mappings_dir)
            .map(|o| o.map(|m| LoadedMappings::Yarn(Arc::new(m))))
            .map_err(LoadError::Yarn),
        MappingType::Vanilla => load_vanilla_mappings(version, mappings_dir)
            .map(|o| o.map(|m| LoadedMappings::Vanilla(Arc::new(m))))
            .map_err(LoadError::Vanilla),
    }
}

/// Deobfuscate `content` against an already-loaded mapping set (zero-copy).
pub fn deobfuscate(loaded: &LoadedMappings, content: &str) -> DeobfuscateResult {
    match loaded {
        LoadedMappings::Yarn(m) => LineEngine::from_arc(Arc::clone(m)).deobfuscate(content),
        LoadedMappings::Vanilla(m) => VanillaEngine::from_arc(Arc::clone(m)).deobfuscate(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::vanilla::parse_tsrg;
    use std::io::Write;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("spinyarn-dispatch-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_mapping_type_parse() {
        assert_eq!(MappingType::parse("vanilla"), MappingType::Vanilla);
        assert_eq!(MappingType::parse("yarn"), MappingType::Yarn);
        assert_eq!(MappingType::parse(""), MappingType::Yarn);
        assert_eq!(MappingType::parse("srg"), MappingType::Yarn);
        assert_eq!(MappingType::Yarn.cache_key("1.21.4"), "yarn:1.21.4");
        assert_eq!(MappingType::Vanilla.cache_key("1.21.4"), "vanilla:1.21.4");
    }

    #[test]
    fn test_load_and_deobfuscate_vanilla() {
        let dir = tmp_dir("vanilla");
        std::fs::create_dir_all(dir.join("vanilla")).unwrap();
        let mut f = std::fs::File::create(dir.join("vanilla").join("1.21.4.txt")).unwrap();
        f.write_all(b"com.example.Main -> a:\n    0:10:void init() -> b\n").unwrap();

        let loaded = load("1.21.4", dir.to_str().unwrap(), MappingType::Vanilla)
            .unwrap()
            .expect("should load");
        let r = deobfuscate(&loaded, "at a.b(SourceFile.java:3)");
        assert_eq!(r.text, "at com.example.Main.init(SourceFile.java:3)");

        // wrong type -> not found
        assert!(load("1.21.4", dir.to_str().unwrap(), MappingType::Yarn)
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_parse_tsrg_via_dispatcher_path() {
        let m = parse_tsrg("com.x.Y -> z:\n    0:5:void run() -> q\n").unwrap();
        assert_eq!(m.lookup_method("z", "q", Some(1)), Some("run"));
    }

    #[test]
    fn test_remove_local_refuses_traversal() {
        let dir = tmp_dir("traversal");
        // A valid version file is removed.
        std::fs::write(dir.join("1.21.9.tiny.gz"), b"x").unwrap();
        assert!(remove_local("1.21.9", dir.to_str().unwrap(), MappingType::Yarn));
        assert!(!dir.join("1.21.9.tiny.gz").exists());

        // A traversal version must be a no-op (and not delete anything).
        assert!(!remove_local("../../etc/passwd", dir.to_str().unwrap(), MappingType::Yarn));
        assert!(remove_all_local("..", dir.to_str().unwrap()).is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
