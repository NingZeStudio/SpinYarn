dnl config.m4 for the spinyarn PHP extension

PHP_ARG_ENABLE([spinyarn],
  [whether to enable spinyarn support],
  [AS_HELP_STRING([--enable-spinyarn],
    [Enable spinyarn support])],
  [no])

if test "$PHP_SPINYARN" != "no"; then
  dnl Locate libspinyarn_capi. Prefer an explicit SPINYARN_LIBDIR, then the
  dnl Rust target/release directory relative to the extension source.
  AC_MSG_CHECKING([for libspinyarn_capi location])

  if test -z "$SPINYARN_LIBDIR"; then
    SPINYARN_LIBDIR="$srcdir/../../target/release"
  fi

  PHP_ADD_LIBRARY_WITH_PATH([spinyarn_capi], [$SPINYARN_LIBDIR], SPINYARN_SHARED_LIBADD)
  PHP_ADD_INCLUDE([$srcdir/../capi/include])

  PHP_SUBST(SPINYARN_SHARED_LIBADD)

  PHP_NEW_EXTENSION([spinyarn], [spinyarn.c], [$ext_shared])
fi
