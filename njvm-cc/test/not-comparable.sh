# The cases of chibicc's test set whose -E or --dump-ast output cannot match the
# vendor compiler's, and why. A byte comparison there reports a design decision
# rather than a defect, so the cases are named and counted apart -- never
# silently dropped.
#
# Sourced by test.sh and bootstrap-emit.sh; call not_comparable <file.c> and it
# prints the reason and answers 0 when the case is one of these.
not_comparable() {
  case "$1" in
    stdhdr.c|varargs.c)
      echo "preprocesses chibicc's <stdarg.h>; njvm-cc's va_list is a struct of its own (see njvm-cc/include/stdarg.h)"
      return 0;;
    macro.c|pragma-once.c|tls.c)
      # These include a path that only resolves with the working directory set to
      # chibicc's own tree, which chibicc's driver does and a guest cannot: its
      # working directory is the host's, fixed at the preopen.
      echo "includes a path that only resolves from chibicc's own tree, and a guest cannot change directory"
      return 0;;
    atomic.c)
      # chibicc's own include directory has no <pthread.h>, so the vendor compiler
      # stops at the first line and emits nothing to compare against. njvm-cc's
      # libc has the atomics and preprocesses the file, which is the behaviour
      # test/e2e/atomics.c covers end to end.
      echo "the vendor compiler has no <pthread.h> and emits nothing; njvm-cc's libc does have it"
      return 0;;
  esac
  return 1
}
