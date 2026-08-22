#ifndef PHP_SPINYARN_H
#define PHP_SPINYARN_H

extern zend_module_entry spinyarn_module_entry;
#define phpext_spinyarn_ptr &spinyarn_module_entry

#define PHP_SPINYARN_VERSION "1.0.0-pre.2"

#if defined(ZTS) && defined(COMPILE_DL_SPINYARN)
ZEND_TSRMLS_CACHE_EXTERN()
#endif

#endif /* PHP_SPINYARN_H */
