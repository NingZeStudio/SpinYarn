use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// Parsed Mojang official mappings (TSRG format), oriented for deobfuscation.
///
/// Vanilla obfuscated **method/field names are NOT globally unique** (short
/// names like `a`/`b` repeat across classes), so members are indexed per class,
/// and methods additionally by their TSRG line-range (to disambiguate overloads).
/// Class names are globally unique.
#[derive(Debug, Clone, Default)]
pub struct VanillaMappings {
    /// obfuscated class name -> readable class name
    classes: HashMap<String, String>,
    /// readable class name -> obfuscated class name (reverse lookup)
    class_by_named: HashMap<String, String>,
    /// readable class name set, to confirm an already-readable class path
    classes_named: HashSet<String>,
    /// class(obf) -> method(obf) -> (start line, end line, readable name)
    methods_by_class: HashMap<String, HashMap<String, Vec<(u32, u32, String)>>>,
    /// class(obf) -> field(obf) -> readable name
    fields_by_class: HashMap<String, HashMap<String, String>>,
}

impl VanillaMappings {
    /// Resolve a class path segment: obfuscated -> readable, or confirm an
    /// already-readable name exists in the mapping.
    pub fn lookup_class<'a>(&'a self, name: &'a str) -> Option<Cow<'a, str>> {
        if let Some(named) = self.classes.get(name) {
            return Some(Cow::Borrowed(named));
        }
        if self.classes_named.contains(name) {
            return Some(Cow::Borrowed(name));
        }
        None
    }

    /// Normalize a class key (obfuscated or readable) to its obfuscated form.
    fn class_obf<'a>(&'a self, class_key: &'a str) -> Option<Cow<'a, str>> {
        if self.classes.contains_key(class_key) {
            return Some(Cow::Borrowed(class_key));
        }
        self.class_by_named
            .get(class_key)
            .map(|s| Cow::Borrowed(s.as_str()))
    }

    /// Resolve a method name within a class, preferring the line-range index
    /// when a line is available; falls back to the first entry for that name.
    pub fn lookup_method(&self, class_key: &str, name: &str, line: Option<u32>) -> Option<&str> {
        let class_obf = self.class_obf(class_key)?;
        let ranges = self
            .methods_by_class
            .get(class_obf.as_ref())?
            .get(name)?;
        if let Some(line) = line {
            for (start, end, named) in ranges {
                if line >= *start && line <= *end {
                    return Some(named);
                }
            }
        }
        Some(&ranges[0].2)
    }

    pub fn lookup_field(&self, class_key: &str, name: &str) -> Option<&str> {
        let class_obf = self.class_obf(class_key)?;
        self.fields_by_class
            .get(class_obf.as_ref())?
            .get(name)
            .map(|s| s.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VanillaParseError {
    #[error("Invalid TSRG line: {0}")]
    InvalidLine(String),
}

/// Parse Mojang official mappings in TSRG format.
///
/// Line shapes:
/// - class:    `<readable> -> <obfuscated>:`
/// - method:   `<start>:<end>:<returnType> <readable>(<args>) -> <obfuscated>`
/// - field:    `<type> <readable> -> <obfuscated>`
/// - comment:  `# ...` or `    # ...`
///
/// Note the direction is `readable -> obfuscated`; we store the reverse
/// (`obfuscated -> readable`) for lookup.
pub fn parse_tsrg(input: &str) -> Result<VanillaMappings, VanillaParseError> {
    let mut m = VanillaMappings::default();
    let mut cur_class: Option<String> = None;

    for raw in input.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            let body = line.trim_start();
            let Some(class_obf) = cur_class.as_deref() else {
                continue;
            };
            if let Some((start, end, named, obf)) = parse_method_line(body) {
                m.methods_by_class
                    .entry(class_obf.to_string())
                    .or_default()
                    .entry(obf)
                    .or_default()
                    .push((start, end, named));
            } else if let Some((named, obf)) = parse_field_line(body) {
                m.fields_by_class
                    .entry(class_obf.to_string())
                    .or_default()
                    .insert(obf, named);
            }
            // Unknown member shape: skip (tolerant parsing).
        } else if let Some((named, obf)) = parse_class_line(trimmed) {
            m.classes.insert(obf.clone(), named.clone());
            m.class_by_named.insert(named.clone(), obf.clone());
            m.classes_named.insert(named);
            cur_class = Some(obf);
        }
    }

    Ok(m)
}

fn parse_class_line(s: &str) -> Option<(String, String)> {
    // `<readable> -> <obfuscated>:` (split at the last separator)
    let rest = s.strip_suffix(':')?;
    let (named, obf) = rest.rsplit_once(" -> ")?;
    Some((named.trim().to_string(), obf.trim().to_string()))
}

fn parse_method_line(s: &str) -> Option<(u32, u32, String, String)> {
    // `<start>:<end>:<returnType> <readable>(<args>) -> <obfuscated>`
    let mut it = s.splitn(3, ':');
    let start: u32 = it.next()?.parse().ok()?;
    let end: u32 = it.next()?.parse().ok()?;
    let rest = it.next()?;
    let (lhs, obf) = rest.rsplit_once(" -> ")?;
    let obf = obf.trim();
    let open = lhs.find('(')?;
    let head = lhs[..open].trim();
    let readable = head.rsplit_once(' ').map(|(_, n)| n).unwrap_or(head);
    Some((start, end, readable.to_string(), obf.to_string()))
}

fn parse_field_line(s: &str) -> Option<(String, String)> {
    // `<type> <readable> -> <obfuscated>`
    let (lhs, obf) = s.rsplit_once(" -> ")?;
    let obf = obf.trim();
    let (_, readable) = lhs.trim().rsplit_once(' ')?;
    Some((readable.to_string(), obf.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# (c) test mappings
com.example.Main -> a:
    0:10:void init() -> b
    5:8:java.lang.String getName() -> c
    11:20:int size() -> d
    int count -> e
    int VALUE -> f
    com.example.Main$Inner inner -> g
com.example.Other -> h:
    0:5:void init() -> b
    java.lang.Object value -> i
";

    #[test]
    fn test_parse_counts() {
        let m = parse_tsrg(SAMPLE).unwrap();
        assert_eq!(m.classes.len(), 2);
        // two classes both have an obf method `b` (not globally unique)
        assert_eq!(m.methods_by_class["a"].len(), 3);
        assert_eq!(m.methods_by_class["h"].len(), 1);
        assert_eq!(m.fields_by_class["a"].len(), 3);
    }

    #[test]
    fn test_lookup_class_both_directions() {
        let m = parse_tsrg(SAMPLE).unwrap();
        assert_eq!(m.lookup_class("a").as_deref(), Some("com.example.Main"));
        assert_eq!(
            m.lookup_class("com.example.Main").as_deref(),
            Some("com.example.Main")
        );
        assert_eq!(m.lookup_class("unknown"), None);
    }

    #[test]
    fn test_method_scoped_by_class_and_line() {
        let m = parse_tsrg(SAMPLE).unwrap();
        // class a, obf method b -> init
        assert_eq!(m.lookup_method("a", "b", Some(3)), Some("init"));
        // same obf name `b` in class h is a different method
        assert_eq!(m.lookup_method("h", "b", Some(2)), Some("init"));
        // readable class key also resolves
        assert_eq!(
            m.lookup_method("com.example.Main", "b", Some(3)),
            Some("init")
        );
        // line outside range -> first entry
        assert_eq!(m.lookup_method("a", "d", Some(30)), Some("size"));
        // unknown class/method
        assert_eq!(m.lookup_method("a", "nope", None), None);
        assert_eq!(m.lookup_method("zzz", "b", None), None);
    }

    #[test]
    fn test_field_scoped_by_class() {
        let m = parse_tsrg(SAMPLE).unwrap();
        assert_eq!(m.lookup_field("a", "f"), Some("VALUE"));
        assert_eq!(m.lookup_field("a", "g"), Some("inner"));
        assert_eq!(m.lookup_field("h", "i"), Some("value"));
        // same obf field name in another class must not leak
        assert_eq!(m.lookup_field("h", "f"), None);
    }

    #[test]
    fn test_constructor() {
        let m = parse_tsrg("com.x.Y -> z:\n    0:5:void <init>() -> <init>\n").unwrap();
        assert_eq!(m.lookup_method("z", "<init>", Some(2)), Some("<init>"));
    }

    #[test]
    #[ignore]
    fn probe_real_client_txt() {
        let content = std::fs::read_to_string("/data/data/com.termux/files/usr/tmp/opencode/client.txt")
            .expect("client.txt missing");
        // Count how many member lines parse_field_line accepts in isolation.
        let mut field_parse = 0;
        let mut method_parse = 0;
        for raw in content.lines() {
            let line = raw.trim_end_matches('\r');
            let s = line.trim();
            if s.is_empty() || s.starts_with('#') {
                continue;
            }
            if line.starts_with(' ') {
                if parse_method_line(line.trim_start()).is_some() {
                    method_parse += 1;
                } else if parse_field_line(line.trim_start()).is_some() {
                    field_parse += 1;
                }
            }
        }
        // Track how many fields are dropped due to cur_class being None.
        let mut cur: Option<String> = None;
        let mut orphan_field = 0;
        for raw in content.lines() {
            let line = raw.trim_end_matches('\r');
            let s = line.trim();
            if s.is_empty() || s.starts_with('#') {
                continue;
            }
            if line.starts_with(' ') {
                let body = line.trim_start();
                if cur.is_none() && parse_field_line(body).is_some() {
                    orphan_field += 1;
                }
            } else if let Some((_, obf)) = parse_class_line(s) {
                cur = Some(obf);
            }
        }
        println!("orphan fields (no current class): {}", orphan_field);

        let m = parse_tsrg(&content).unwrap();
        let total_methods: usize = m
            .methods_by_class
            .values()
            .map(|mm| mm.values().map(|v| v.len()).sum::<usize>())
            .sum();
        let total_fields: usize = m.fields_by_class.values().map(|fm| fm.len()).sum();
        println!(
            "single-pass: methods={} fields={} | parsed: classes={} methods={} fields={}",
            method_parse, field_parse, m.classes.len(), total_methods, total_fields
        );
        assert_eq!(total_methods, method_parse, "methods dropped");
        // Fields may collide on the same (class, obf) key (983 duplicates in
        // 1.21.4); HashMap::insert keeps the last mapping, so unique keys < rows.
        assert!(
            total_fields <= field_parse && total_fields >= field_parse - 1200,
            "fields: unique={} rows={}",
            total_fields,
            field_parse
        );
        assert_eq!(m.lookup_class("fda").as_deref(), Some("com.mojang.blaze3d.Blaze3D"));
        assert_eq!(m.lookup_method("fda", "a", Some(9)), Some("youJustLostTheGame"));
        // readable class key + line-range disambiguation
        assert_eq!(
            m.lookup_method("com.mojang.blaze3d.Blaze3D", "b", Some(13)),
            Some("getTime")
        );
    }
}
