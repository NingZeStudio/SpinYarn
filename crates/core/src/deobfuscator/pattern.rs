use once_cell::sync::Lazy;
use regex::Regex;

/// Residual catch-all pattern for non-stack lines.
///
/// Two alternatives:
/// - bare `class_/method_/field_` keys, guarded by a leading non-word char
///   (prevents `Xclass_310` false matches) and a trailing `\b`;
/// - `net[./]minecraft[./]class_X` with an explicit prefix (tolerates the `L`
///   of descriptors like `Lnet/minecraft/class_X;`).
///
/// Greedy `\d+` naturally wins over prefix conflicts (`class_31` vs `class_310`).
/// Class keys allow a nested `$` suffix (`class_1$class_2`, possibly repeated).
pub static RESIDUAL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?:(?P<pre>^|[^\w$])(?P<key1>class_\d+(?:\$class_\d+)*|method_\d+|field_\d+)|(?P<prefix>net[./]minecraft[./])(?P<key2>class_\d+(?:\$class_\d+)*))\b",
    )
    .expect("residual pattern compiles")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nested_dollar_suffix_captured_whole() {
        // M4：嵌套 `$` 后缀应作为整体捕获，而非截断在内层 `class_` 前。
        let caps = RESIDUAL_PATTERN
            .captures("oops class_11980$class_11981 broken")
            .expect("should match");
        let key = caps
            .name("key1")
            .or_else(|| caps.name("key2"))
            .expect("has key")
            .as_str();
        assert_eq!(key, "class_11980$class_11981");
    }
}
