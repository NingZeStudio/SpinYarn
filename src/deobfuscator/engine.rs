use std::collections::HashMap;

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
    classes: HashMap<String, String>,
    methods: HashMap<String, String>,
    fields: HashMap<String, String>,
    nested: HashMap<String, String>,
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
            classes: mappings.classes,
            methods: mappings.methods,
            fields: mappings.fields,
            nested: mappings.nested,
        }
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
            let text = if newline { &line[..line.len() - 1] } else { line };
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

        let open = body.rfind('(')?;
        let close = body.rfind(')')?;
        if close <= open {
            return None;
        }
        let paren = &body[open + 1..close];
        let call = &body[..open];
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
        let mut new_method = method.to_string();
        if method.starts_with("method_") {
            if let Some(named) = self.methods.get(method) {
                new_method = named.clone();
                method_hit = true;
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

        Some((out, class_hit, method_hit))
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
            .classes
            .get(key)
            .or_else(|| self.nested.get(key))
            .or_else(|| key.rfind('$').and_then(|d| self.classes.get(&key[..d])))
        else {
            return paren.to_string();
        };

        let short = named.rsplit('/').next().unwrap_or(named);
        format!("{}{}{}{}", &file[..pos], short, &file[pos + key.len()..], line)
    }

    /// Look up an obfuscated class key embedded in a class path segment.
    /// Handles `class_310`, `class_11980$class_11981`, and falls back to the
    /// outer class when an anonymous/nested-class key is not present in the table.
    fn lookup_class(&self, class_part: &str) -> Option<&str> {
        // Find the FIRST `class_` so nested keys like `class_11980$class_11981`
        // are captured whole rather than truncated at the inner `class_`.
        let pos = class_part.find("class_")?;
        let rest = &class_part[pos..];
        let end = rest
            .bytes()
            .take_while(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
            .count();
        let key = &rest[..end];
        if key.len() <= "class_".len() {
            return None;
        }

        if let Some(named) = self.classes.get(key) {
            return Some(named);
        }
        if let Some(named) = self.nested.get(key) {
            return Some(named);
        }
        if let Some(dollar) = key.rfind('$') {
            return self.classes.get(&key[..dollar]).map(|s| s.as_str());
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
            let m = caps.get(0).unwrap();
            let pre = caps.name("pre").map(|p| p.as_str()).unwrap_or("");
            let key = caps
                .name("key1")
                .or_else(|| caps.name("key2"))
                .unwrap()
                .as_str();
            let prefix = caps.name("prefix").map(|p| p.as_str()).unwrap_or("");

            let replacement = if key.starts_with("class_") {
                self.classes.get(key).or_else(|| self.nested.get(key)).map(|n| {
                    if prefix.contains('.') {
                        n.replace('/', ".")
                    } else {
                        n.clone()
                    }
                })
            } else if key.starts_with("method_") {
                self.methods.get(key).cloned()
            } else {
                self.fields.get(key).cloned()
            };

            let Some(rep) = replacement else { continue };

            if key.starts_with("class_") {
                classes += 1;
            } else if key.starts_with("method_") {
                methods += 1;
            } else {
                fields += 1;
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
