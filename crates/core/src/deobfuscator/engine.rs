use std::sync::Arc;

use crate::deobfuscator::pattern::RESIDUAL_PATTERN;
use crate::mapping::Mappings;

#[derive(Debug, Clone, PartialEq)]
pub struct DeobfuscateResult {
    pub text: String,
    pub classes_mapped: usize,
    pub methods_mapped: usize,
    pub fields_mapped: usize,
    pub total_time_ms: f64,
}

/// High-performance per-version deobfuscation engine.
///
/// Strategy:
/// - Stack lines are parsed by hand (memchr-based) and remapped structurally.
/// - Non-stack lines fall back to a single precompiled regex pass.
/// - Lookup tables are global (`method_XXXX` / `field_XXXX` are globally unique).
/// - The engine owns no cache; build once per request and drop.
pub struct LineEngine {
    mappings: Arc<Mappings>,
}

fn strip_module_prefix(class_part: &str) -> &str {
    // Word chars followed by one or more slashes form a module prefix
    // (`knot//`, `knot/`). A dot breaks it, so paths like `java.base/...`
    // keep their prefix intact.
    let bytes = class_part.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i > 0 && i < bytes.len() && bytes[i] == b'/' {
        while i < bytes.len() && bytes[i] == b'/' {
            i += 1;
        }
        return &class_part[i..];
    }
    class_part
}

impl LineEngine {
    pub fn new(mappings: Mappings) -> Self {
        Self {
            mappings: Arc::new(mappings),
        }
    }

    /// Build from an already-shared mapping table (e.g. a cache hit).
    pub fn from_arc(mappings: Arc<Mappings>) -> Self {
        Self { mappings }
    }

    pub fn deobfuscate(&self, input: &str) -> DeobfuscateResult {
        use std::time::Instant;

        let start = Instant::now();
        let mut classes_mapped = 0usize;
        let mut methods_mapped = 0usize;
        let mut fields_mapped = 0usize;

        let mut out = String::with_capacity(input.len());

        for line in input.split_inclusive('\n') {
            let newline = line.ends_with('\n');
            let text = if newline {
                &line[..line.len() - 1]
            } else {
                line
            };
            let text = text.strip_suffix('\r').unwrap_or(text);

            if let Some((mapped, ch, mh)) = self.map_stack_line(text) {
                classes_mapped += ch as usize;
                methods_mapped += mh as usize;
                out.push_str(&mapped);
            } else if let Some((mapped, ch, mh, fh)) = self.residual_replace(text) {
                classes_mapped += ch;
                methods_mapped += mh;
                fields_mapped += fh;
                out.push_str(&mapped);
            } else {
                out.push_str(text);
            }

            if newline {
                out.push('\n');
            }
        }

        let total_time_ms = start.elapsed().as_secs_f64() * 1000.0;

        DeobfuscateResult {
            text: out,
            classes_mapped,
            methods_mapped,
            fields_mapped,
            total_time_ms,
        }
    }

    /// Structured remap of a single stack-trace line:
    /// `   at <class>.<method>(<file>:<line>)<suffix>`
    /// Returns `None` if the line is not a remappable stack line (or nothing matched).
    fn map_stack_line(&self, line: &str) -> Option<(String, bool, bool)> {
        let lead = line
            .bytes()
            .take_while(|&b| b == b' ' || b == b'\t')
            .count();
        let content = &line[lead..];
        if !content.starts_with("at ") {
            return None;
        }
        let body = &content[3..];

        // L4：用第一个 '(' 定位 call 起点（`rfind` 会误吞 suffix 中的括号），
        // 并要求 call 内无空白/括号，否则走降级路径。
        let open = body.find('(')?;
        let close = body.rfind(')')?;
        if close <= open {
            return None;
        }
        let paren = &body[open + 1..close];
        let call = &body[..open];
        if call.contains(' ') || call.contains('\t') || call.contains('(') || call.contains(')') {
            return None;
        }
        let suffix = &body[close + 1..];

        let dot = call.rfind('.')?;
        let class_part = &call[..dot];
        let method = &call[dot + 1..];
        if class_part.is_empty() || method.is_empty() {
            return None;
        }

        let mut class_hit = false;
        let mut new_class_part = class_part.to_string();
        if let Some(named) = self.lookup_class(class_part) {
            new_class_part = named.replace('/', ".");
            class_hit = true;
        }

        let mut method_hit = false;
        let mut method_counted = false;
        let mut new_method = method.to_string();
        if method.starts_with("method_") {
            if let Some(named) = self.mappings.methods.get(method) {
                new_method = named.clone();
                method_hit = true;
                // Yarn 未命名方法（named == key）仍参与重建（去模块前缀、
                // 替换文件名），但不计数：计入会虚增 methods_mapped。
                method_counted = named != method;
            }
        }

        if !class_hit && !method_hit {
            return None;
        }

        // Class name not mapped (usually already readable) but a method was:
        // drop any module prefix like `knot//`/`knot/` so the rebuilt line
        // uses the plain class path.
        if !class_hit {
            new_class_part = strip_module_prefix(class_part).to_string();
        }

        let new_paren = self.replace_file_name(paren);

        let mut out = String::with_capacity(line.len() + 16);
        out.push_str(&line[..lead]);
        out.push_str("at ");
        out.push_str(&new_class_part);
        out.push('.');
        out.push_str(&new_method);
        out.push('(');
        out.push_str(&new_paren);
        out.push(')');
        out.push_str(suffix);

        // 诚实计数：类名替换前后文本一致（如输入已是可读全名）时不计数，
        // 但重建照常（方法/文件名可能仍有变化）。
        let class_counted = class_hit && new_class_part != class_part;
        Some((out, class_counted, method_counted))
    }

    /// Remap an obfuscated class name inside the `(file:line)` part of a stack
    /// line, e.g. `class_310.java:465` -> `MinecraftClient.java:465`.
    fn replace_file_name(&self, paren: &str) -> String {
        let (file, line) = match paren.rfind(':') {
            Some(i) => (&paren[..i], &paren[i..]),
            None => (paren, ""),
        };

        let Some(pos) = file.find("class_") else {
            return paren.to_string();
        };
        let rest = &file[pos..];
        let end = rest
            .bytes()
            .take_while(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
            .count();
        let key = &rest[..end];
        if key.len() <= "class_".len() {
            return paren.to_string();
        }

        let Some(named) = self
            .mappings
            .classes
            .get(key)
            .or_else(|| self.mappings.nested.get(key))
            .or_else(|| {
                key.rfind('$')
                    .and_then(|d| self.mappings.classes.get(&key[..d]))
            })
        else {
            return paren.to_string();
        };

        let short = named.rsplit('/').next().unwrap_or(named);
        format!(
            "{}{}{}{}",
            &file[..pos],
            short,
            &file[pos + key.len()..],
            line
        )
    }

    /// Look up an obfuscated class key embedded in a class path segment.
    /// Handles `class_310`, `class_11980$class_11981`, and falls back to the
    /// outer class when an anonymous/nested-class key is not present in the table.
    /// L5：`class_` 前须为行首/`/`/`.`/`$`，键体严格为 `class_\d+` 加 `$` 嵌套，
    /// 与 residual 正则键形状一致（如 `Xclass_1`、`class_abc` 不命中）。
    fn lookup_class(&self, class_part: &str) -> Option<&str> {
        let bytes = class_part.as_bytes();
        let mut from = 0usize;
        while let Some(rel) = class_part[from..].find("class_") {
            let pos = from + rel;
            // 前导字符检查。
            if pos > 0 && !matches!(bytes[pos - 1], b'.' | b'/' | b'$') {
                from = pos + "class_".len();
                continue;
            }
            // 键体：`class_` 后至少一位数字。
            let mut end = pos + "class_".len();
            let digits = end;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end == digits {
                from = pos + "class_".len();
                continue;
            }
            // 嵌套后缀：`$class_数字` 可重复出现。
            loop {
                if end + "$class_".len() <= bytes.len()
                    && &class_part[end..end + "$class_".len()] == "$class_"
                {
                    let mut d = end + "$class_".len();
                    let ds = d;
                    while d < bytes.len() && bytes[d].is_ascii_digit() {
                        d += 1;
                    }
                    if d == ds {
                        break;
                    }
                    end = d;
                } else {
                    break;
                }
            }
            let key = &class_part[pos..end];
            // 键后紧跟单词字符说明是更长标识符的一部分（如 `class_12x`），跳过。
            if end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                from = pos + "class_".len();
                continue;
            }

            if let Some(named) = self.mappings.classes.get(key) {
                return Some(named);
            }
            if let Some(named) = self.mappings.nested.get(key) {
                return Some(named);
            }
            if let Some(dollar) = key.rfind('$') {
                return self
                    .mappings
                    .classes
                    .get(&key[..dollar])
                    .map(|s| s.as_str());
            }
            return None;
        }
        None
    }

    /// Residual regex pass for non-stack lines (descriptors, messages, etc.).
    /// Returns the remapped line and match counts, or `None` when nothing matched.
    fn residual_replace(&self, line: &str) -> Option<(String, usize, usize, usize)> {
        // Fast path: the regex is only needed when the line actually carries an
        // obfuscated key prefix. Most real log lines don't, so skip the regex
        // engine entirely and let the caller push the line through unchanged.
        // `contains` uses memchr-style scanning, far cheaper than a regex pass.
        if !line.contains("class_") && !line.contains("method_") && !line.contains("field_") {
            return None;
        }

        let mut out = String::with_capacity(line.len());
        let mut last = 0usize;
        let mut changed = false;
        let mut classes = 0usize;
        let mut methods = 0usize;
        let mut fields = 0usize;

        for caps in RESIDUAL_PATTERN.captures_iter(line) {
            let m = caps.get(0).expect("regex 全匹配存在");
            let pre = caps.name("pre").map(|p| p.as_str()).unwrap_or("");
            let key = caps
                .name("key1")
                .or_else(|| caps.name("key2"))
                .expect("key1/key2 必居其一")
                .as_str();
            let prefix = caps.name("prefix").map(|p| p.as_str()).unwrap_or("");

            let replacement = if key.starts_with("class_") {
                // M4：先查全键（含 `$` 嵌套），miss 再回退外层，与 lookup_class 一致。
                self.mappings
                    .classes
                    .get(key)
                    .or_else(|| self.mappings.nested.get(key))
                    .or_else(|| {
                        key.rfind('$')
                            .and_then(|d| self.mappings.classes.get(&key[..d]))
                    })
                    .map(|n| {
                        if prefix.contains('.') {
                            n.replace('/', ".")
                        } else {
                            n.clone()
                        }
                    })
            } else if key.starts_with("method_") {
                self.mappings.methods.get(key).cloned()
            } else {
                self.mappings.fields.get(key).cloned()
            };

            let Some(rep) = replacement else { continue };

            // 诚实计数：替换前后文本一致（Yarn 未命名条目）时不计数，
            // 但替换照做（输出字节与过去完全一致）。
            if rep != key {
                if key.starts_with("class_") {
                    classes += 1;
                } else if key.starts_with("method_") {
                    methods += 1;
                } else {
                    fields += 1;
                }
            }
            changed = true;
            out.push_str(&line[last..m.start()]);
            out.push_str(pre);
            out.push_str(&rep);
            last = m.end();
        }

        if !changed {
            return None;
        }
        out.push_str(&line[last..]);
        Some((out, classes, methods, fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn engine_with(classes: &[(&str, &str)], methods: &[(&str, &str)]) -> LineEngine {
        let m = Mappings {
            classes: classes
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
            methods: methods
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
            fields: HashMap::new(),
            nested: HashMap::new(),
        };
        LineEngine::new(m)
    }

    #[test]
    fn test_residual_nested_full_key_and_outer_fallback() {
        // M4：全键命中与 miss 回退外层。
        let e = engine_with(
            &[
                ("class_10$class_11", "com/Outer$Inner"),
                ("class_20", "com/Outer"),
            ],
            &[],
        );
        let r = e.deobfuscate("oops class_10$class_11 broken");
        assert!(r.text.contains("com/Outer$Inner"), "全键应命中：{}", r.text);
        let r2 = e.deobfuscate("oops class_20$class_99 broken");
        assert!(
            r2.text.contains("com/Outer"),
            "未知内层应回退外层：{}",
            r2.text
        );
    }

    #[test]
    fn test_stack_line_uses_first_paren() {
        // L4：suffix 含括号时仍按第一个 '(' 定位 call。
        let e = engine_with(&[("class_1", "com/A")], &[("method_1", "foo")]);
        let r = e.deobfuscate("at class_1.method_1(Source.java:1) (extra)");
        assert_eq!(r.text, "at com.A.foo(Source.java:1) (extra)");
    }

    #[test]
    fn test_stack_line_call_with_space_falls_back() {
        // L4：call 含空白时不走结构化映射（不 panic，结果由残留路径决定）。
        let e = engine_with(&[("class_1", "com/A")], &[]);
        assert!(e.map_stack_line("at class_1.foo bar(Foo.java:1)").is_none());
    }

    #[test]
    fn test_self_mapped_counts_zero_but_line_rebuilt() {
        // M3（引擎层诚实计数）：自映射方法仍触发重建（去模块前缀），但不计数。
        let e = engine_with(&[("class_1", "com/A")], &[("method_1", "method_1")]);
        let r = e.deobfuscate("at knot//com.A.method_1(Foo.java:1)");
        assert_eq!(r.text, "at com.A.method_1(Foo.java:1)");
        assert_eq!(r.classes_mapped, 0);
        assert_eq!(r.methods_mapped, 0);
        // 残留路径同样：文本照换（此处无变化），计数为 0。
        let r2 = e.deobfuscate("oops method_1 broken");
        assert_eq!(r2.text, "oops method_1 broken");
        assert_eq!(r2.methods_mapped, 0);
    }

    #[test]
    fn test_lookup_class_rejects_bad_prefix_and_shape() {
        // L5：`Xclass_1`（前导单词字符）与 `class_abc`（非数字键体）不命中。
        let e = engine_with(&[("class_1", "com/A")], &[]);
        assert_eq!(e.lookup_class("Xclass_1"), None);
        assert_eq!(e.lookup_class("class_abc"), None);
        assert_eq!(e.lookup_class("class_1"), Some("com/A"));
        // 栈行侧同样透传。
        let line = "at Xclass_1.foo(Foo.java:1)";
        assert_eq!(e.deobfuscate(line).text, line);
    }
}
