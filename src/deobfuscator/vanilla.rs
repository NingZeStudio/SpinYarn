use std::sync::Arc;

use crate::deobfuscator::engine::DeobfuscateResult;
use crate::mapping::vanilla::VanillaMappings;

/// Structured stack-line deobfuscator for Vanilla (Mojang official) mappings.
///
/// Vanilla obfuscated names are single-character short names that cannot be
/// safely matched by a residual regex (any plain text like `a.b` would match),
/// so lines are only rewritten when their class resolves in the mapping AND the
/// method resolves within that class (optionally by the TSRG line range to
/// disambiguate overloads). Unmapped lines pass through untouched.
pub struct VanillaEngine {
    mappings: Arc<VanillaMappings>,
}

impl VanillaEngine {
    pub fn new(mappings: VanillaMappings) -> Self {
        Self {
            mappings: Arc::new(mappings),
        }
    }

    pub fn from_arc(mappings: Arc<VanillaMappings>) -> Self {
        Self { mappings }
    }

    pub fn deobfuscate(&self, input: &str) -> DeobfuscateResult {
        use std::time::Instant;

        let start = Instant::now();
        let mut classes_mapped = 0usize;
        let mut methods_mapped = 0usize;

        let mut out = String::with_capacity(input.len());

        for line in input.split_inclusive('\n') {
            let newline = line.ends_with('\n');
            let text = if newline { &line[..line.len() - 1] } else { line };
            let text = text.strip_suffix('\r').unwrap_or(text);

            if let Some((mapped, ch, mh)) = self.map_stack_line(text) {
                classes_mapped += ch as usize;
                methods_mapped += mh as usize;
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
            // Vanilla obfuscated field names are short and not globally unique,
            // so they cannot be safely rewritten — this count is always 0.
            fields_mapped: 0,
            total_time_ms,
        }
    }

    /// Remap a single stack line `at <class>.<method>(<file>:<line>)` when both
    /// the class and method resolve in the Vanilla mapping.
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

        // Class must resolve (obfuscated -> readable, or already-readable in map).
        let (named_class, class_hit) = match self.mappings.lookup_class(class_part) {
            Some(c) => (c.into_owned(), true),
            None => return None,
        };

        // Method resolves within the class; unmapped methods keep their name.
        let line_num = parse_line_number(paren);
        let method_mapped = self
            .mappings
            .lookup_method(class_part, method, line_num)
            .map(|s| s.to_string());
        let named_method = method_mapped.clone().unwrap_or_else(|| method.to_string());

        let mut out = String::with_capacity(line.len() + 16);
        out.push_str(&line[..lead]);
        out.push_str("at ");
        out.push_str(&named_class);
        out.push('.');
        out.push_str(&named_method);
        out.push('(');
        out.push_str(paren);
        out.push(')');
        out.push_str(suffix);

        Some((out, class_hit, method_mapped.is_some()))
    }
}

/// Extract the line number from the `(file:line)` part of a stack frame.
fn parse_line_number(paren: &str) -> Option<u32> {
    let (_file, line) = paren.rsplit_once(':')?;
    line.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_from_tsrg(s: &str) -> VanillaEngine {
        VanillaEngine::new(crate::mapping::vanilla::parse_tsrg(s).unwrap())
    }

    const MAP: &str = "\
com.example.Main -> a:
    0:10:void init() -> b
    5:8:java.lang.String getName() -> c
    11:20:int size() -> d
com.example.Other -> h:
    0:5:void init() -> b
";

    #[test]
    fn test_stack_line_obf_class_and_method() {
        let e = engine_from_tsrg(MAP);
        let r = e.deobfuscate("at a.b(SourceFile.java:3)");
        assert_eq!(r.text, "at com.example.Main.init(SourceFile.java:3)");
        assert_eq!(r.classes_mapped, 1);
        assert_eq!(r.methods_mapped, 1);
    }

    #[test]
    fn test_method_line_disambiguation() {
        let e = engine_from_tsrg(MAP);
        // b maps to init for class a
        assert_eq!(e.deobfuscate("at a.b(SourceFile.java:3)").text, "at com.example.Main.init(SourceFile.java:3)");
    }

    #[test]
    fn test_readable_class_kept_when_mapped() {
        let e = engine_from_tsrg(MAP);
        // already-readable class confirmed in mapping, obf method resolved
        let r = e.deobfuscate("at com.example.Main.b(SourceFile.java:3)");
        assert_eq!(r.text, "at com.example.Main.init(SourceFile.java:3)");
    }

    #[test]
    fn test_unmapped_class_passthrough() {
        let e = engine_from_tsrg(MAP);
        // class not in mapping -> whole line untouched (no regex guessing)
        let line = "at zz.x(SourceFile.java:3)";
        assert_eq!(e.deobfuscate(line).text, line);
    }

    #[test]
    fn test_unmapped_method_keeps_name() {
        let e = engine_from_tsrg(MAP);
        // class resolves, method `xyz` not in map -> class rewritten, method kept
        let r = e.deobfuscate("at a.xyz(SourceFile.java:3)");
        assert_eq!(r.text, "at com.example.Main.xyz(SourceFile.java:3)");
        assert_eq!(r.classes_mapped, 1);
        assert_eq!(r.methods_mapped, 0);
    }

    #[test]
    fn test_non_stack_lines_passthrough() {
        let e = engine_from_tsrg(MAP);
        let log = "[00:00:01] [main/INFO]: Starting\njava.lang.RuntimeException: oops\n";
        assert_eq!(e.deobfuscate(log).text, log);
    }

    #[test]
    fn test_unknown_source_no_line() {
        let e = engine_from_tsrg(MAP);
        // (Unknown Source) has no line number -> method resolves via first range
        let r = e.deobfuscate("at a.b(Unknown Source)");
        assert_eq!(r.text, "at com.example.Main.init(Unknown Source)");
    }
}
