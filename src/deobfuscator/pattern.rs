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
pub static RESIDUAL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?:(?P<pre>^|[^\w$])(?P<key1>class_\d+|method_\d+|field_\d+)|(?P<prefix>net[./]minecraft[./])(?P<key2>class_\d+))\b",
    )
    .unwrap()
});
