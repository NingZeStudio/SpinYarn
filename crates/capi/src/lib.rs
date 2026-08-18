//! SpinYarn C ABI.
//!
//! Thin FFI layer over `spinyarn-core`. Every exported function is
//! `#[no_mangle] pub extern "C"` and panic-safe: a panicking Rust call is
//! caught and converted to a NULL/error return rather than unwinding across
//! the FFI boundary (which would be UB).
//!
//! Type/variant names follow C `snake_case`/`SCREAMING_SNAKE` conventions to
//! match `spinyarn.h`, hence the `allow` attributes below.

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
// C ABI entry points inherently dereference raw pointers passed by callers;
// `extern "C"` functions can't be `unsafe fn`, so this lint is waived here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::panic::{catch_unwind, AssertUnwindSafe};

use spinyarn_core::config::Config;
use spinyarn_core::mapping::dispatcher::MappingType;
use spinyarn_core::Spinyarn;

/// Opaque engine handle: wraps the synchronous `Spinyarn` facade.
pub struct spinyarn_handle {
    inner: Spinyarn,
}

/// Maximum accepted `content_len` for a single deobfuscation call, aligned with
/// the Web API's `DEFAULT_MAX_BODY_SIZE`. Guards against a misbehaving host
/// passing a bogus huge length that would cause an out-of-bounds read or a
/// large allocation.
const MAX_CONTENT_LEN: usize = 64 * 1024 * 1024;

/// Opaque result: owns the deobfuscated text plus per-pass counters.
pub struct spinyarn_result {
    text: CString,
    classes: usize,
    methods: usize,
    fields: usize,
    time_ms: f64,
}

/// Mapping family discriminant (must match `spinyarn.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum spinyarn_mapping_type_t {
    SPINYARN_YARN = 0,
    SPINYARN_VANILLA = 1,
}

/// Convert a `MappingType` discriminant into the Rust enum.
fn to_mapping_type(t: spinyarn_mapping_type_t) -> MappingType {
    match t {
        spinyarn_mapping_type_t::SPINYARN_VANILLA => MappingType::Vanilla,
        _ => MappingType::Yarn,
    }
}

/// # Safety
/// `mappings_dir` must be a valid NUL-terminated C string (or NULL to use the
/// `SPINYARN_MAPPINGS_DIR`/`exe_dir()` default). `auto_download` is 0/1.
/// Uses the default LRU cache bound.
#[no_mangle]
pub extern "C" fn spinyarn_init(
    mappings_dir: *const c_char,
    auto_download: c_int,
) -> *mut spinyarn_handle {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let dir = resolve_dir(mappings_dir);
        let inner = Spinyarn::from_settings(&dir, auto_download != 0);
        Box::into_raw(Box::new(spinyarn_handle { inner }))
    }));
    match result {
        Ok(handle) => handle,
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `mappings_dir` must be a valid NUL-terminated C string (or NULL to use the
/// `SPINYARN_MAPPINGS_DIR`/`exe_dir()` default). `auto_download` is 0/1.
///
/// Full MySQLi-style positional config:
/// - `cache_max_entries`: 0 = disable the LRU cache; a positive value caps the
///   cache at that many entries.
/// - `cache_high_watermark` / `cache_low_watermark`: 0 = auto (derived from the
///   cap); otherwise used verbatim.
#[no_mangle]
pub extern "C" fn spinyarn_init_full(
    mappings_dir: *const c_char,
    auto_download: c_int,
    cache_max_entries: usize,
    cache_high_watermark: usize,
    cache_low_watermark: usize,
) -> *mut spinyarn_handle {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let dir = resolve_dir(mappings_dir);
        let inner = Spinyarn::from_full_settings(
            &dir,
            auto_download != 0,
            cache_max_entries,
            cache_high_watermark,
            cache_low_watermark,
        );
        Box::into_raw(Box::new(spinyarn_handle { inner }))
    }));
    match result {
        Ok(handle) => handle,
        Err(_) => std::ptr::null_mut(),
    }
}

/// Resolve the mappings dir from a C string (NULL → Config default).
fn resolve_dir(mappings_dir: *const c_char) -> String {
    if mappings_dir.is_null() {
        return Config::default().maven.mappings_dir.clone();
    }
    unsafe { std::ffi::CStr::from_ptr(mappings_dir) }
        .to_string_lossy()
        .into_owned()
}

/// # Safety
/// `handle` must be NULL or a pointer returned by `spinyarn_init` that has not
/// yet been freed.
#[no_mangle]
pub extern "C" fn spinyarn_free(handle: *mut spinyarn_handle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(handle) });
    }));
}

/// # Safety
/// `handle` must be a valid pointer from `spinyarn_init`; `content` must be a
/// valid pointer to `content_len` bytes; `version` must be a valid
/// NUL-terminated C string.
#[no_mangle]
pub extern "C" fn spinyarn_deobfuscate(
    handle: *mut spinyarn_handle,
    content: *const c_char,
    content_len: usize,
    version: *const c_char,
    mapping_type: spinyarn_mapping_type_t,
) -> *mut spinyarn_result {
    if handle.is_null() || content.is_null() || version.is_null() {
        return std::ptr::null_mut();
    }
    if content_len == 0 || content_len > MAX_CONTENT_LEN {
        return std::ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { &*handle };
        let version = unsafe { std::ffi::CStr::from_ptr(version) }
            .to_string_lossy()
            .into_owned();
        let content = unsafe {
            std::str::from_utf8(std::slice::from_raw_parts(content.cast::<u8>(), content_len))
                .unwrap_or("")
        };
        let out = handle.inner.deobfuscate(content, &version, to_mapping_type(mapping_type));
        // A log can contain NUL bytes in principle; CString stops at the first
        // NUL, so truncate the payload to the first NUL to keep the C contract
        // well-defined. Real MC logs are NUL-free.
        let text = match out.deobfuscated.find('\0') {
            Some(idx) => &out.deobfuscated[..idx],
            None => out.deobfuscated.as_str(),
        };
        Box::into_raw(Box::new(spinyarn_result {
            text: CString::new(text).unwrap_or_default(),
            classes: out.classes_mapped,
            methods: out.methods_mapped,
            fields: out.fields_mapped,
            time_ms: out.total_time_ms,
        }))
    }));
    result.unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `result` must be a valid pointer from `spinyarn_deobfuscate`.
#[no_mangle]
pub extern "C" fn spinyarn_result_text(result: *const spinyarn_result) -> *const c_char {
    if result.is_null() {
        return std::ptr::null();
    }
    unsafe { (*result).text.as_ptr() }
}

/// # Safety
/// `result` must be a valid pointer from `spinyarn_deobfuscate`.
#[no_mangle]
pub extern "C" fn spinyarn_result_len(result: *const spinyarn_result) -> usize {
    if result.is_null() {
        return 0;
    }
    unsafe { (*result).text.as_bytes().len() }
}

/// # Safety
/// `result` must be a valid pointer from `spinyarn_deobfuscate`.
#[no_mangle]
pub extern "C" fn spinyarn_result_classes(result: *const spinyarn_result) -> usize {
    if result.is_null() {
        return 0;
    }
    unsafe { (*result).classes }
}

/// # Safety
/// `result` must be a valid pointer from `spinyarn_deobfuscate`.
#[no_mangle]
pub extern "C" fn spinyarn_result_methods(result: *const spinyarn_result) -> usize {
    if result.is_null() {
        return 0;
    }
    unsafe { (*result).methods }
}

/// # Safety
/// `result` must be a valid pointer from `spinyarn_deobfuscate`.
#[no_mangle]
pub extern "C" fn spinyarn_result_fields(result: *const spinyarn_result) -> usize {
    if result.is_null() {
        return 0;
    }
    unsafe { (*result).fields }
}

/// # Safety
/// `result` must be a valid pointer from `spinyarn_deobfuscate`.
#[no_mangle]
pub extern "C" fn spinyarn_result_time_ms(result: *const spinyarn_result) -> f64 {
    if result.is_null() {
        return 0.0;
    }
    unsafe { (*result).time_ms }
}

/// # Safety
/// `result` must be NULL or a pointer from `spinyarn_deobfuscate` not yet freed.
#[no_mangle]
pub extern "C" fn spinyarn_result_free(result: *mut spinyarn_result) {
    if result.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(result) });
    }));
}

/// # Safety
/// `handle` must be a valid pointer from `spinyarn_init`; `version` must be a
/// valid NUL-terminated C string.
#[no_mangle]
pub extern "C" fn spinyarn_load_mapping(
    handle: *mut spinyarn_handle,
    version: *const c_char,
    mapping_type: spinyarn_mapping_type_t,
    force: c_int,
) -> c_int {
    if handle.is_null() || version.is_null() {
        return 0;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { &*handle };
        let version = unsafe { std::ffi::CStr::from_ptr(version) }
            .to_string_lossy()
            .into_owned();
        handle
            .inner
            .load_mapping(&version, to_mapping_type(mapping_type), force != 0)
            .unwrap_or(false)
    }));
    result.unwrap_or(false) as c_int
}

/// # Safety
/// `handle` must be a valid pointer from `spinyarn_init`; `version` must be a
/// valid NUL-terminated C string.
#[no_mangle]
pub extern "C" fn spinyarn_has_mapping(
    handle: *mut spinyarn_handle,
    version: *const c_char,
    mapping_type: spinyarn_mapping_type_t,
) -> c_int {
    if handle.is_null() || version.is_null() {
        return 0;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let handle = unsafe { &*handle };
        let version = unsafe { std::ffi::CStr::from_ptr(version) }
            .to_string_lossy()
            .into_owned();
        handle
            .inner
            .has_mapping(&version, to_mapping_type(mapping_type))
    }));
    result.unwrap_or(false) as c_int
}

/// Library version string, static lifetime.
#[no_mangle]
pub extern "C" fn spinyarn_version() -> *const c_char {
    static VERSION: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| CString::new(env!("CARGO_PKG_VERSION")).unwrap())
        .as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_version() {
        let v = unsafe { std::ffi::CStr::from_ptr(spinyarn_version()) };
        assert_eq!(v.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_mapping_type_discriminants() {
        assert_eq!(spinyarn_mapping_type_t::SPINYARN_YARN as c_int, 0);
        assert_eq!(spinyarn_mapping_type_t::SPINYARN_VANILLA as c_int, 1);
    }

    #[test]
    fn test_init_free_null_safe() {
        spinyarn_free(std::ptr::null_mut());
        assert!(spinyarn_result_text(std::ptr::null()).is_null());
        assert_eq!(spinyarn_result_len(std::ptr::null()), 0);
    }

    #[test]
    fn test_init_and_passthrough() {
        let handle = spinyarn_init(std::ptr::null(), 0);
        assert!(!handle.is_null());

        // A non-downloadable version (snapshot) passes through unchanged.
        let content = CString::new("at net.minecraft.class_1234.method_5678(X.java:1)").unwrap();
        let version = CString::new("25w44a").unwrap();
        let result = spinyarn_deobfuscate(
            handle,
            content.as_ptr(),
            content.as_bytes().len(),
            version.as_ptr(),
            spinyarn_mapping_type_t::SPINYARN_YARN,
        );
        assert!(!result.is_null());
        let text = unsafe { std::ffi::CStr::from_ptr(spinyarn_result_text(result)) };
        assert_eq!(text.to_str().unwrap(), "at net.minecraft.class_1234.method_5678(X.java:1)");
        assert_eq!(spinyarn_result_classes(result), 0);
        spinyarn_result_free(result);

        spinyarn_free(handle);
    }

    #[test]
    fn test_deobfuscate_null_args() {
        let handle = spinyarn_init(std::ptr::null(), 0);
        assert!(spinyarn_deobfuscate(
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            spinyarn_mapping_type_t::SPINYARN_YARN
        )
        .is_null());
        spinyarn_free(handle);
    }
}
