use std::collections::HashMap;

/// Global deobfuscation lookup tables (intermediary -> named).
/// Only entries whose keys match the obfuscated `class_`/`method_`/`field_`
/// prefix are kept; entries already using readable official names (e.g. `run`,
/// `add`) are dropped to avoid corrupting arbitrary log text.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Mappings {
    pub classes: HashMap<String, String>,
    pub methods: HashMap<String, String>,
    pub fields: HashMap<String, String>,
    /// Reverse lookup for nested-class bare keys: inner `class_XXXX` -> full
    /// named name. Logs sometimes reference the inner key of a nested class
    /// without its outer prefix (e.g. bare `class_7512` for
    /// `class_2874$class_7512`). Only populated for inner keys that are
    /// globally unique across all nested classes, to avoid ambiguity.
    pub nested: HashMap<String, String>,
}

impl Mappings {
    pub fn classes_len(&self) -> usize {
        self.classes.len()
    }
    pub fn methods_len(&self) -> usize {
        self.methods.len()
    }
    pub fn fields_len(&self) -> usize {
        self.fields.len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TinyV2ParseError {
    #[error("Invalid header: expected 'tiny\\t2' or 'v1'")]
    InvalidHeader,

    #[error("Missing namespaces in header")]
    MissingNamespaces,

    #[error("Invalid class line format")]
    InvalidClassLine,

    #[error("Invalid field line format")]
    InvalidFieldLine,

    #[error("Invalid method line format")]
    InvalidMethodLine,

    #[error("Namespace '{0}' not found in header")]
    NamespaceNotFound(String),
}

/// Header 列数上限：正常 tiny 头仅 4~6 列，超限视为畸形输入。
/// 防止恶意超长 header 导致 `ns + 2` 等定位值远超 `cols` 长度。
const MAX_HEADER_COLS: usize = 64;

/// `collect_cols` 返回的栈上列数组长度（`take(6)`）。
const MAX_COLS: usize = 6;

/// Namespace column positions within a header line (absolute indices).
#[derive(Debug, Clone)]
struct NamespacePositions {
    #[allow(dead_code)]
    official: usize,
    intermediary: usize,
    named: usize,
}

impl NamespacePositions {
    fn parse(header: &str) -> Result<Self, TinyV2ParseError> {
        let parts: Vec<&str> = header.split('\t').collect();
        if parts.len() > MAX_HEADER_COLS {
            return Err(TinyV2ParseError::InvalidHeader);
        }

        let find = |name: &str| -> Result<usize, TinyV2ParseError> {
            parts
                .iter()
                .position(|p| *p == name)
                .ok_or_else(|| TinyV2ParseError::NamespaceNotFound(name.to_string()))
        };

        let official = find("official")?;
        Ok(Self {
            official,
            intermediary: find("intermediary")?,
            named: find("named")?,
        })
    }

    fn v1_class_col(&self, ns: usize) -> Option<usize> {
        Some(ns).filter(|&v| v < MAX_COLS)
    }
    fn v1_member_col(&self, ns: usize) -> Option<usize> {
        ns.checked_add(2).filter(|&v| v < MAX_COLS)
    }
    fn v2_class_col(&self, ns: usize) -> Option<usize> {
        ns.checked_sub(2).filter(|&v| v < MAX_COLS)
    }
    fn v2_member_col(&self, ns: usize) -> Option<usize> {
        ns.checked_sub(1).filter(|&v| v < MAX_COLS)
    }
}

fn normalize_key(key: &str) -> &str {
    key.strip_prefix("net/minecraft/")
        .or_else(|| key.strip_prefix("net.minecraft."))
        .unwrap_or(key)
}

/// Split a line into up to 6 tab-separated columns onto the stack.
/// Avoids the per-line heap allocation of `collect::<Vec<&str>>()`.
#[inline]
fn collect_cols(line: &str) -> [Option<&str>; 6] {
    let mut out = [None; 6];
    for (idx, f) in line.split('\t').take(6).enumerate() {
        out[idx] = Some(f);
    }
    out
}

/// Rough pre-allocation for the lookup tables based on line count.
/// 上限 1M 条目：`line_count` 来自输入长度计数，不设上限时畸形超大输入
/// 会一次性申请巨大 HashMap 而 OOM。
fn prealloc(line_count: usize) -> Mappings {
    const CAP: usize = 1 << 20;
    Mappings {
        classes: HashMap::with_capacity(std::cmp::min(line_count / 10, CAP)),
        methods: HashMap::with_capacity(std::cmp::min(line_count / 2, CAP)),
        fields: HashMap::with_capacity(std::cmp::min(line_count / 2, CAP)),
        nested: HashMap::new(),
    }
}

/// Collect nested-class candidates: inner key (after the last `$`) -> full
/// named names seen for it. Duplicate inner keys are dropped later because
/// they are ambiguous.
fn collect_nested(
    m: &mut Mappings,
    key: &str,
    named: &str,
    candidates: &mut HashMap<String, Vec<String>>,
) {
    m.classes.insert(key.to_owned(), named.to_owned());
    if let Some(dollar) = key.rfind('$') {
        let inner = &key[dollar + 1..];
        if inner.starts_with("class_") {
            candidates
                .entry(inner.to_owned())
                .or_default()
                .push(named.to_owned());
        }
    }
}

/// Populate the `nested` reverse table from candidates, keeping only inner
/// keys that appeared exactly once across all nested classes.
fn finalize_nested(m: &mut Mappings, candidates: HashMap<String, Vec<String>>) {
    for (inner, names) in candidates {
        if names.len() == 1 {
            m.nested.insert(
                inner,
                names.into_iter().next().expect("len == 1 has one name"),
            );
        }
    }
}

/// Parse a Tiny v1 or Tiny v2 mapping file into global lookup tables.
/// Format is auto-detected from the header line.
pub fn parse(input: &[u8]) -> Result<Mappings, TinyV2ParseError> {
    let s = std::str::from_utf8(input).map_err(|_| TinyV2ParseError::InvalidHeader)?;
    let header = s
        .lines()
        .next()
        .ok_or(TinyV2ParseError::InvalidHeader)?
        .trim_end_matches('\r')
        .trim();

    if header.starts_with("tiny\t2") {
        parse_v2(s)
    } else if header.starts_with("v1") {
        parse_v1(s)
    } else {
        Err(TinyV2ParseError::InvalidHeader)
    }
}

/// Tiny v1: flat lines, `CLASS`/`FIELD`/`METHOD`.
///   CLASS  <name-ns1> <name-ns2> ...
///   FIELD  <parent-ns1> <desc-ns1> <name-ns1> <name-ns2> ...
///   METHOD <parent-ns1> <desc-ns1> <name-ns1> <name-ns2> ...
/// The parent class column is ignored (global tables do not need it).
fn parse_v1(input: &str) -> Result<Mappings, TinyV2ParseError> {
    let line_count = input.as_bytes().iter().filter(|&&b| b == b'\n').count() + 1;
    let mut m = prealloc(line_count);
    let mut nested_candidates: HashMap<String, Vec<String>> = HashMap::new();
    let mut lines = input.lines();
    let header = lines
        .next()
        .expect("parse()已校验首行存在")
        .trim_end_matches('\r');
    let ns = NamespacePositions::parse(header)?;

    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        let cols = collect_cols(line);
        match cols[0] {
            Some("CLASS") => {
                let (Some(i), Some(n)) =
                    (ns.v1_class_col(ns.intermediary), ns.v1_class_col(ns.named))
                else {
                    return Err(TinyV2ParseError::InvalidClassLine);
                };
                let (Some(ik), Some(named)) = (cols[i], cols[n]) else {
                    return Err(TinyV2ParseError::InvalidClassLine);
                };
                let key = normalize_key(ik);
                if !key.starts_with("class_") {
                    continue;
                }
                collect_nested(&mut m, key, named, &mut nested_candidates);
            }
            Some("FIELD") => {
                let (Some(i), Some(n)) = (
                    ns.v1_member_col(ns.intermediary),
                    ns.v1_member_col(ns.named),
                ) else {
                    return Err(TinyV2ParseError::InvalidFieldLine);
                };
                let (Some(ik), Some(named)) = (cols[i], cols[n]) else {
                    return Err(TinyV2ParseError::InvalidFieldLine);
                };
                let key = normalize_key(ik);
                if !key.starts_with("field_") {
                    continue;
                }
                m.fields.insert(key.to_owned(), named.to_owned());
            }
            Some("METHOD") => {
                let (Some(i), Some(n)) = (
                    ns.v1_member_col(ns.intermediary),
                    ns.v1_member_col(ns.named),
                ) else {
                    return Err(TinyV2ParseError::InvalidMethodLine);
                };
                let (Some(ik), Some(named)) = (cols[i], cols[n]) else {
                    return Err(TinyV2ParseError::InvalidMethodLine);
                };
                let key = normalize_key(ik);
                if !key.starts_with("method_") {
                    continue;
                }
                m.methods.insert(key.to_owned(), named.to_owned());
            }
            _ => continue, // skip comments / properties
        }
    }

    finalize_nested(&mut m, nested_candidates);
    Ok(m)
}

/// Tiny v2: hierarchical (tab indentation), `c`/`f`/`m` sections.
///   c  <name-ns1> <name-ns2> ...
///   \tf <desc-ns1> <name-ns1> <name-ns2> ...
///   \tm <desc-ns1> <name-ns1> <name-ns2> ...
fn parse_v2(input: &str) -> Result<Mappings, TinyV2ParseError> {
    let line_count = input.as_bytes().iter().filter(|&&b| b == b'\n').count() + 1;
    let mut m = prealloc(line_count);
    let mut nested_candidates: HashMap<String, Vec<String>> = HashMap::new();
    let mut ns: Option<NamespacePositions> = None;

    for (line_num, line) in input.lines().enumerate() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            continue;
        }

        if line_num == 0 {
            let header = trimmed.trim();
            if header.split('\t').count() < 5 {
                return Err(TinyV2ParseError::MissingNamespaces);
            }
            ns = Some(NamespacePositions::parse(header)?);
            continue;
        }

        let ns = ns.as_ref().ok_or(TinyV2ParseError::InvalidHeader)?;
        let leading = trimmed.len() - trimmed.trim_start_matches('\t').len();
        let content = &trimmed[leading..];
        let cols = collect_cols(content);
        if cols[0].is_none() || cols[0] == Some("") {
            continue;
        }

        match cols[0] {
            Some("c") if leading == 0 => {
                let (Some(i), Some(n)) =
                    (ns.v2_class_col(ns.intermediary), ns.v2_class_col(ns.named))
                else {
                    return Err(TinyV2ParseError::InvalidClassLine);
                };
                let (Some(ik), Some(named)) = (cols[i], cols[n]) else {
                    return Err(TinyV2ParseError::InvalidClassLine);
                };
                let key = normalize_key(ik);
                if !key.starts_with("class_") {
                    continue;
                }
                collect_nested(&mut m, key, named, &mut nested_candidates);
            }
            Some("f") if leading == 1 => {
                let (Some(i), Some(n)) = (
                    ns.v2_member_col(ns.intermediary),
                    ns.v2_member_col(ns.named),
                ) else {
                    return Err(TinyV2ParseError::InvalidFieldLine);
                };
                let (Some(ik), Some(named)) = (cols[i], cols[n]) else {
                    return Err(TinyV2ParseError::InvalidFieldLine);
                };
                let key = normalize_key(ik);
                if !key.starts_with("field_") {
                    continue;
                }
                m.fields.insert(key.to_owned(), named.to_owned());
            }
            Some("m") if leading == 1 => {
                let (Some(i), Some(n)) = (
                    ns.v2_member_col(ns.intermediary),
                    ns.v2_member_col(ns.named),
                ) else {
                    return Err(TinyV2ParseError::InvalidMethodLine);
                };
                let (Some(ik), Some(named)) = (cols[i], cols[n]) else {
                    return Err(TinyV2ParseError::InvalidMethodLine);
                };
                let key = normalize_key(ik);
                if !key.starts_with("method_") {
                    continue;
                }
                m.methods.insert(key.to_owned(), named.to_owned());
            }
            _ => continue, // skip comments, params, vars, properties
        }
    }

    finalize_nested(&mut m, nested_candidates);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v1_member_col_out_of_bounds_returns_error_not_panic() {
        // M1：header 把 named 推到远超 6 列，`ns + 2` 越界时应返回错误而非 panic。
        let mut header = String::from("v1\tofficial\tintermediary");
        for i in 0..10 {
            header.push_str(&format!("\tpad{}", i));
        }
        header.push_str("\tnamed");
        let input = format!("{}\nCLASS\tofficialX\tclass_1\tcom.Example\n", header);
        let r = parse(input.as_bytes());
        assert!(r.is_err(), "越界列应返回错误");
    }

    #[test]
    fn test_header_too_many_columns_rejected() {
        // M1：header 列数超过上限直接拒绝。
        let mut header = String::from("tiny\t2\t0");
        for i in 0..70 {
            header.push_str(&format!("\tns{}", i));
        }
        let input = format!("{}\n", header);
        assert!(matches!(
            parse(input.as_bytes()),
            Err(TinyV2ParseError::InvalidHeader)
        ));
    }

    #[test]
    fn test_prealloc_capped_for_huge_line_count() {
        // L2：超大 line_count 下预分配应被 cap 住而非按 line_count/2 申请。
        let m = prealloc(1usize << 32);
        assert!(m.methods.capacity() <= (1 << 20) * 2);
        assert!(m.fields.capacity() <= (1 << 20) * 2);
        assert!(m.classes.capacity() <= (1 << 20) * 2);
    }
}
