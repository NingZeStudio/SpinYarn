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

/*
 * Create an engine from a config path. Pass NULL to use the built-in defaults
 * (mappings dir from SPINYARN_MAPPINGS_DIR or <exe>/mappings, auto_download
 * on). Returns NULL on failure.
 */
spinyarn_handle_t *spinyarn_init(const char *config_path);

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

/* Deobfuscated UTF-8 text and its byte length. */
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
