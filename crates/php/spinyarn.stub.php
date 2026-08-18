<?php

/** @generate-class-entries */

/**
 * Initialize a SpinYarn engine handle (MySQLi-style positional config).
 *
 * No config file: all settings are passed positionally. The engine is released
 * automatically when the returned resource is garbage-collected (no manual free
 * needed).
 *
 * @param string|null $mappings_dir       Directory holding the mapping files; NULL
 *                                        falls back to SPINYARN_MAPPINGS_DIR or the
 *                                        host executable's ./mappings.
 * @param bool $auto_download             Download missing mappings on demand (1.x
 *                                        series only; snapshots/26.x excluded).
 * @param int  $cache_max_entries         0 = disable the LRU cache; a positive value
 *                                        caps it at that many entries. Default 44.
 * @param int  $cache_high_watermark      High watermark for batch eviction; 0 = auto
 *                                        (derived from the cap). Default 40.
 * @param int  $cache_low_watermark       Low watermark to evict down to; 0 = auto.
 *                                        Default 30.
 * @return resource|false                 Engine handle resource, or false on failure.
 */
function spinyarn_init(?string $mappings_dir = null, bool $auto_download = true, int $cache_max_entries = 44, int $cache_high_watermark = 40, int $cache_low_watermark = 30) {}

/**
 * Deobfuscate a log's stack traces.
 *
 * @param resource $handle      Handle from spinyarn_init().
 * @param string   $content     The (possibly multi-line) log text.
 * @param string   $version     Minecraft version (e.g. "1.21.9", "1.18.2-pre1").
 * @param int      $mapping_type SPINYARN_YARN (0, default) or SPINYARN_VANILLA (1).
 * @return array|false          Assoc array: deobfuscated, classes_mapped,
 *                              methods_mapped, fields_mapped, total_time_ms.
 *                              Unavailable mappings pass through unchanged with
 *                              zero counters. false on invalid handle/failure.
 */
function spinyarn_deobfuscate($handle, string $content, string $version, int $mapping_type = SPINYARN_YARN) {}

/**
 * Load/refresh a version's mapping file from its source.
 *
 * @param resource $handle      Handle from spinyarn_init().
 * @param string   $version     Minecraft version.
 * @param int      $mapping_type SPINYARN_YARN (0) or SPINYARN_VANILLA (1).
 * @param bool     $force       Force re-download even if fresh (default false).
 * @return bool                 true if the mapping is ready locally.
 */
function spinyarn_load_mapping($handle, string $version, int $mapping_type = SPINYARN_YARN, bool $force = false) {}

/**
 * Whether a version/type mapping file exists locally.
 *
 * @param resource $handle      Handle from spinyarn_init().
 * @param string   $version     Minecraft version.
 * @param int      $mapping_type SPINYARN_YARN (0) or SPINYARN_VANILLA (1).
 * @return bool
 */
function spinyarn_has_mapping($handle, string $version, int $mapping_type = SPINYARN_YARN) {}

/**
 * Library version string (e.g. "1.0.0-pre.1").
 *
 * @return string
 */
function spinyarn_version() {}

/**
 * Bootstrap the default full version list (43 Yarn + Vanilla families): download
 * every missing mapping file into the mappings dir. Synchronous/blocking — call
 * from an init/deploy path, not the hot request path.
 *
 * @param resource $handle      Handle from spinyarn_init().
 * @return int|false            Number of mapping files downloaded (>= 0), or
 *                              false on an invalid handle.
 */
function spinyarn_bootstrap($handle) {}
