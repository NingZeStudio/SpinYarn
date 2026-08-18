#ifndef SPINYARN_H
#define SPINYARN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque engine handle. */
typedef struct spinyarn_handle spinyarn_handle_t;

/* Result of a deobfuscation pass. */
typedef struct spinyarn_result spinyarn_result_t;

/* Mapping family. */
typedef enum {
    SPINYARN_YARN = 0,
    SPINYARN_VANILLA = 1,
} spinyarn_mapping_type_t;

/* Compile-time guard: the discriminants must match the Rust #[repr(C)] enum
 * in spinyarn-capi (spinyarn_mapping_type_t) and the PHP extension constants. */
#if defined(__cplusplus)
static_assert(SPINYARN_YARN == 0 && SPINYARN_VANILLA == 1,
              "spinyarn_mapping_type_t discriminants must be 0/1");
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(SPINYARN_YARN == 0 && SPINYARN_VANILLA == 1,
               "spinyarn_mapping_type_t discriminants must be 0/1");
#endif

/*
 * Create an engine from explicit settings (no config file).
 * - `mappings_dir`: directory holding the mapping files. Pass NULL to fall back
 *   to SPINYARN_MAPPINGS_DIR or <exe>/mappings.
 * - `auto_download`: 1 to download missing mappings on demand, 0 to disable.
 * Returns NULL on failure.
 */
spinyarn_handle_t *spinyarn_init(const char *mappings_dir, int auto_download);

/*
 * Same as spinyarn_init, but lets the host control the LRU cache.
 * - `cache_max_entries`: 0 = disable the cache; a positive value caps it at
 *   that many entries. Use this from PHP-FPM-style multi-worker hosts to bound
 *   per-process memory.
 */
spinyarn_handle_t *spinyarn_init_ext(const char *mappings_dir, int auto_download,
                                     size_t cache_max_entries);

/* Release the engine and all associated resources. */
void spinyarn_free(spinyarn_handle_t *handle);

/*
 * Deobfuscate `content` (UTF-8) against `version` / `mapping_type`.
 * The content need not be NUL-terminated; length is explicit.
 * Returns a result (never NULL for a valid handle); on an unavailable
 * mapping the input is returned unchanged (passthrough).
 */
spinyarn_result_t *spinyarn_deobfuscate(
    spinyarn_handle_t *handle,
    const char *content,
    size_t content_len,
    const char *version,
    spinyarn_mapping_type_t mapping_type);

/*
 * Deobfuscated UTF-8 text and its byte length.
 *
 * NOTE: the text is truncated at the first NUL byte (0x00) if the input log
 * contains one, since `spinyarn_result_text` returns a NUL-terminated C string.
 * Real Minecraft logs are NUL-free, so this is a non-issue in practice; use
 * `spinyarn_result_len` as the authoritative length (it reflects the truncated
 * payload, not the original).
 */
const char *spinyarn_result_text(const spinyarn_result_t *result);
size_t spinyarn_result_len(const spinyarn_result_t *result);

/* Per-pass counters (0 on passthrough). */
size_t spinyarn_result_classes(const spinyarn_result_t *result);
size_t spinyarn_result_methods(const spinyarn_result_t *result);
size_t spinyarn_result_fields(const spinyarn_result_t *result);
double spinyarn_result_time_ms(const spinyarn_result_t *result);

/* Release a result. */
void spinyarn_result_free(spinyarn_result_t *result);

/* Load/refresh a version's mapping from its source (1 = ready). */
int spinyarn_load_mapping(
    spinyarn_handle_t *handle,
    const char *version,
    spinyarn_mapping_type_t mapping_type,
    int force);

/* Whether a version/type mapping file exists locally (1 = yes). */
int spinyarn_has_mapping(
    spinyarn_handle_t *handle,
    const char *version,
    spinyarn_mapping_type_t mapping_type);

/* Library version string (e.g. "0.9.0"). */
const char *spinyarn_version(void);

#ifdef __cplusplus
}
#endif

#endif /* SPINYARN_H */
