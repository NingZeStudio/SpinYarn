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
 * @param int  $cache_max_entries         0 = disable the LRU cache; a positive value
 *                                        caps it at that many entries. Default 44.
 * @param int  $cache_high_watermark      High watermark for batch eviction; 0 = auto
 *                                        (derived from the cap). Default 40.
 * @param int  $cache_low_watermark       Low watermark to evict down to; 0 = auto.
 *                                        Default 30.
 * @return resource|false                 Engine handle resource, or false on failure.
 */
function spinyarn_init(?string $mappings_dir = null, int $cache_max_entries = 44, int $cache_high_watermark = 40, int $cache_low_watermark = 30) {}

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
 * Whether a version/type mapping file exists locally.
 *
 * @param resource $handle      Handle from spinyarn_init().
 * @param string   $version     Minecraft version.
 * @param int      $mapping_type SPINYARN_YARN (0) or SPINYARN_VANILLA (1).
 * @return bool
 */
function spinyarn_has_mapping($handle, string $version, int $mapping_type = SPINYARN_YARN) {}

/**
 * Library version string (e.g. "1.0.0-pre.2").
 *
 * @return string
 */
function spinyarn_version() {}
