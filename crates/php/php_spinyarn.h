#ifndef PHP_SPINYARN_H
#define PHP_SPINYARN_H

extern zend_module_entry spinyarn_module_entry;
#define phpext_spinyarn_ptr &spinyarn_module_entry

#define PHP_SPINYARN_VERSION "0.9.0"

#if defined(ZTS) && defined(COMPILE_DL_SPINYARN)
ZEND_TSRMLS_CACHE_EXTERN()
#endif

#endif /* PHP_SPINYARN_H */
