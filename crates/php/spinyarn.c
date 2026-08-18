/* SpinYarn PHP extension.
 *
 * Thin Zend API wrapper over the SpinYarn C ABI (libspinyarn_capi.so).
 * Each `spinyarn_init()` returns a PHP resource wrapping an opaque
 * `spinyarn_handle_t*`; the resource destructor releases it via
 * `spinyarn_free`. `spinyarn_deobfuscate()` returns an assoc array with the
 * deobfuscated text and per-pass counters.
 */

#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"

#include "spinyarn.h"
#include "php_spinyarn.h"

/* Resource type id for the engine handle. */
static int le_spinyarn;

/* ------------------------------------------------------------------------
 * Handle helpers
 * --------------------------------------------------------------------- */

static void spinyarn_handle_dtor(zend_resource *rsrc)
{
    spinyarn_handle_t *handle = (spinyarn_handle_t *)rsrc->ptr;
    if (handle != NULL) {
        spinyarn_free(handle);
        rsrc->ptr = NULL;
    }
}

/* Fetch and validate a handle resource, returning the raw pointer or NULL. */
static spinyarn_handle_t *fetch_handle(zval *z)
{
    if (Z_TYPE_P(z) != IS_RESOURCE) {
        php_error_docref(NULL, E_WARNING, "expects a Spinyarn handle resource");
        return NULL;
    }
    return (spinyarn_handle_t *)zend_fetch_resource(Z_RES_P(z), "Spinyarn handle", le_spinyarn);
}

/* ------------------------------------------------------------------------
 * PHP functions
 * --------------------------------------------------------------------- */

/* spinyarn_init(?string $mappings_dir = null, bool $auto_download = true,
 *               int $cache_max_entries = 0): resource */
PHP_FUNCTION(spinyarn_init)
{
    zend_string *mappings_dir = NULL;
    bool auto_download = true;
    zend_long cache_max_entries = 0;

    ZEND_PARSE_PARAMETERS_START(0, 3)
        Z_PARAM_OPTIONAL
        Z_PARAM_STR_OR_NULL(mappings_dir)
        Z_PARAM_BOOL(auto_download)
        Z_PARAM_LONG(cache_max_entries)
    ZEND_PARSE_PARAMETERS_END();

    if (cache_max_entries < 0) {
        cache_max_entries = 0;
    }

    spinyarn_handle_t *handle = spinyarn_init_ext(
        mappings_dir ? ZSTR_VAL(mappings_dir) : NULL,
        auto_download ? 1 : 0,
        (size_t)cache_max_entries);
    if (handle == NULL) {
        php_error_docref(NULL, E_WARNING, "failed to initialize Spinyarn");
        RETURN_FALSE;
    }

    RETURN_RES(zend_register_resource(handle, le_spinyarn));
}

/* spinyarn_deobfuscate(resource $handle, string $content,
 *                      string $version, int $mapping_type = 0): array|false */
PHP_FUNCTION(spinyarn_deobfuscate)
{
    zval *handle_z;
    char *content = NULL;
    size_t content_len = 0;
    char *version = NULL;
    size_t version_len = 0;
    zend_long mapping_type = SPINYARN_YARN;

    ZEND_PARSE_PARAMETERS_START(3, 4)
        Z_PARAM_RESOURCE(handle_z)
        Z_PARAM_STRING(content, content_len)
        Z_PARAM_STRING(version, version_len)
        Z_PARAM_OPTIONAL
        Z_PARAM_LONG(mapping_type)
    ZEND_PARSE_PARAMETERS_END();

    spinyarn_handle_t *handle = fetch_handle(handle_z);
    if (handle == NULL) {
        RETURN_FALSE;
    }

    spinyarn_result_t *result = spinyarn_deobfuscate(
        handle, content, content_len, version, (spinyarn_mapping_type_t)mapping_type);
    if (result == NULL) {
        php_error_docref(NULL, E_WARNING, "deobfuscation failed");
        RETURN_FALSE;
    }

    const char *text = spinyarn_result_text(result);
    size_t text_len = spinyarn_result_len(result);
    if (text == NULL) {
        spinyarn_result_free(result);
        php_error_docref(NULL, E_WARNING, "deobfuscation produced no output");
        RETURN_FALSE;
    }

    array_init(return_value);
    add_assoc_stringl(return_value, "deobfuscated", (char *)text, text_len);
    add_assoc_long(return_value, "classes_mapped", (zend_long)spinyarn_result_classes(result));
    add_assoc_long(return_value, "methods_mapped", (zend_long)spinyarn_result_methods(result));
    add_assoc_long(return_value, "fields_mapped", (zend_long)spinyarn_result_fields(result));
    add_assoc_double(return_value, "total_time_ms", spinyarn_result_time_ms(result));

    spinyarn_result_free(result);
}

/* spinyarn_load_mapping(resource $handle, string $version,
 *                       int $mapping_type = 0, bool $force = false): bool */
PHP_FUNCTION(spinyarn_load_mapping)
{
    zval *handle_z;
    char *version = NULL;
    size_t version_len = 0;
    zend_long mapping_type = SPINYARN_YARN;
    bool force = false;

    ZEND_PARSE_PARAMETERS_START(2, 4)
        Z_PARAM_RESOURCE(handle_z)
        Z_PARAM_STRING(version, version_len)
        Z_PARAM_OPTIONAL
        Z_PARAM_LONG(mapping_type)
        Z_PARAM_BOOL(force)
    ZEND_PARSE_PARAMETERS_END();

    spinyarn_handle_t *handle = fetch_handle(handle_z);
    if (handle == NULL) {
        RETURN_FALSE;
    }

    int ok = spinyarn_load_mapping(
        handle, version, (spinyarn_mapping_type_t)mapping_type, force ? 1 : 0);
    RETURN_BOOL(ok != 0);
}

/* spinyarn_has_mapping(resource $handle, string $version,
 *                      int $mapping_type = 0): bool */
PHP_FUNCTION(spinyarn_has_mapping)
{
    zval *handle_z;
    char *version = NULL;
    size_t version_len = 0;
    zend_long mapping_type = SPINYARN_YARN;

    ZEND_PARSE_PARAMETERS_START(2, 3)
        Z_PARAM_RESOURCE(handle_z)
        Z_PARAM_STRING(version, version_len)
        Z_PARAM_OPTIONAL
        Z_PARAM_LONG(mapping_type)
    ZEND_PARSE_PARAMETERS_END();

    spinyarn_handle_t *handle = fetch_handle(handle_z);
    if (handle == NULL) {
        RETURN_FALSE;
    }

    int ok = spinyarn_has_mapping(
        handle, version, (spinyarn_mapping_type_t)mapping_type);
    RETURN_BOOL(ok != 0);
}

/* spinyarn_version(): string */
PHP_FUNCTION(spinyarn_version)
{
    ZEND_PARSE_PARAMETERS_NONE();
    RETURN_STRING(spinyarn_version());
}

/* ------------------------------------------------------------------------
 * Arg info
 * --------------------------------------------------------------------- */

ZEND_BEGIN_ARG_INFO_EX(arginfo_spinyarn_init, 0, 0, 0)
    ZEND_ARG_TYPE_INFO(0, mappings_dir, IS_STRING, 1)
    ZEND_ARG_TYPE_INFO(0, auto_download, _IS_BOOL, 1)
    ZEND_ARG_TYPE_INFO(0, cache_max_entries, IS_LONG, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_spinyarn_deobfuscate, 0, 0, 3)
    ZEND_ARG_INFO(0, handle)
    ZEND_ARG_TYPE_INFO(0, content, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, version, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, mapping_type, IS_LONG, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_spinyarn_load_mapping, 0, 0, 2)
    ZEND_ARG_INFO(0, handle)
    ZEND_ARG_TYPE_INFO(0, version, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, mapping_type, IS_LONG, 1)
    ZEND_ARG_TYPE_INFO(0, force, _IS_BOOL, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_spinyarn_has_mapping, 0, 0, 2)
    ZEND_ARG_INFO(0, handle)
    ZEND_ARG_TYPE_INFO(0, version, IS_STRING, 0)
    ZEND_ARG_TYPE_INFO(0, mapping_type, IS_LONG, 1)
ZEND_END_ARG_INFO()

ZEND_BEGIN_ARG_INFO_EX(arginfo_spinyarn_version, 0, 0, 0)
ZEND_END_ARG_INFO()

/* ------------------------------------------------------------------------
 * Function table
 * --------------------------------------------------------------------- */

static const zend_function_entry spinyarn_functions[] = {
    PHP_FE(spinyarn_init, arginfo_spinyarn_init)
    PHP_FE(spinyarn_deobfuscate, arginfo_spinyarn_deobfuscate)
    PHP_FE(spinyarn_load_mapping, arginfo_spinyarn_load_mapping)
    PHP_FE(spinyarn_has_mapping, arginfo_spinyarn_has_mapping)
    PHP_FE(spinyarn_version, arginfo_spinyarn_version)
    PHP_FE_END
};

/* ------------------------------------------------------------------------
 * Module lifecycle
 * --------------------------------------------------------------------- */

PHP_MINIT_FUNCTION(spinyarn)
{
    le_spinyarn = zend_register_list_destructors_ex(
        spinyarn_handle_dtor, NULL, "Spinyarn handle", module_number);

    REGISTER_LONG_CONSTANT("SPINYARN_YARN", SPINYARN_YARN, CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SPINYARN_VANILLA", SPINYARN_VANILLA, CONST_PERSISTENT);

    return SUCCESS;
}

PHP_MINFO_FUNCTION(spinyarn)
{
    php_info_print_table_start();
    php_info_print_table_row(2, "Spinyarn support", "enabled");
    php_info_print_table_row(2, "Version", PHP_SPINYARN_VERSION);
    php_info_print_table_row(2, "Library", spinyarn_version());
    php_info_print_table_end();
}

zend_module_entry spinyarn_module_entry = {
    STANDARD_MODULE_HEADER,
    "spinyarn",
    spinyarn_functions,
    PHP_MINIT(spinyarn),
    NULL, /* MSHUTDOWN */
    NULL, /* RINIT */
    NULL, /* RSHUTDOWN */
    PHP_MINFO(spinyarn),
    PHP_SPINYARN_VERSION,
    STANDARD_MODULE_PROPERTIES
};

#ifdef COMPILE_DL_SPINYARN
#ifdef ZTS
ZEND_TSRMLS_CACHE_DEFINE()
#endif
ZEND_GET_MODULE(spinyarn)
#endif
