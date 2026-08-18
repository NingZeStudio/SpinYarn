dnl config.m4 for the spinyarn PHP extension

PHP_ARG_ENABLE([spinyarn],
  [whether to enable spinyarn support],
  [AS_HELP_STRING([--enable-spinyarn],
    [Enable spinyarn support])],
  [no])

if test "$PHP_SPINYARN" != "no"; then
  dnl Locate libspinyarn_capi. Prefer an explicit SPINYARN_LIBDIR, then the
  dnl Rust target/release directory relative to the extension source.
  if test -z "$SPINYARN_LIBDIR"; then
    SPINYARN_LIBDIR="$srcdir/../../target/release"
  fi

  dnl Fail early with a clear message when the C ABI library is missing.
  AC_MSG_CHECKING([for libspinyarn_capi in $SPINYARN_LIBDIR])
  if test ! -f "$SPINYARN_LIBDIR/libspinyarn_capi.so" && test ! -f "$SPINYARN_LIBDIR/libspinyarn_capi.a"; then
    AC_MSG_RESULT([not found])
    AC_MSG_ERROR([libspinyarn_capi not found in $SPINYARN_LIBDIR. Build it first with:
  cargo build --release -p spinyarn-capi
Or pass an explicit --with-spinyarn-libdir=/path/to/target/release])
  fi
  AC_MSG_RESULT([found])

  PHP_ADD_LIBRARY_WITH_PATH([spinyarn_capi], [$SPINYARN_LIBDIR], SPINYARN_SHARED_LIBADD)
  PHP_ADD_INCLUDE([$srcdir/../capi/include])

  dnl Verify the C header is present before compiling.
  AC_MSG_CHECKING([for spinyarn.h])
  if test ! -f "$srcdir/../capi/include/spinyarn.h"; then
    AC_MSG_RESULT([not found])
    AC_MSG_ERROR([spinyarn.h header not found at $srcdir/../capi/include])
  fi
  AC_MSG_RESULT([found])

  PHP_SUBST(SPINYARN_SHARED_LIBADD)

  PHP_NEW_EXTENSION([spinyarn], [spinyarn.c], [$ext_shared])
fi
