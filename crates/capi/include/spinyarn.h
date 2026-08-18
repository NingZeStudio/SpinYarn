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
 * Uses the default LRU cache bound.
 * Returns NULL on failure.
 */
spinyarn_handle_t *spinyarn_init(const char *mappings_dir, int auto_download);

/*
 * Full MySQLi-style positional configuration (no config file).
 * - `mappings_dir`: directory holding the mapping files (NULL = default).
 * - `auto_download`: 1 = download missing mappings on demand, 0 = disable.
 * - `cache_max_entries`: 0 = disable the LRU cache; a positive value caps it at
 *   that many entries.
 * - `cache_high_watermark` / `cache_low_watermark`: 0 = auto (derived from the
 *   cap); otherwise used verbatim.
 * Returns NULL on failure.
 */
spinyarn_handle_t *spinyarn_init_full(const char *mappings_dir, int auto_download,
                                      size_t cache_max_entries,
                                      size_t cache_high_watermark,
                                      size_t cache_low_watermark);

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

/* Library version string (e.g. "1.0.0-pre.1"). */
const char *spinyarn_version(void);

/*
 * Bootstrap the default full version list (43 Yarn + Vanilla families): download
 * every missing mapping file. Synchronous (blocking); call from an init/deploy
 * path, not the hot request path. Returns the number of files downloaded (>= 0).
 */
size_t spinyarn_bootstrap(spinyarn_handle_t *handle);

#ifdef __cplusplus
}
#endif

#endif /* SPINYARN_H */
