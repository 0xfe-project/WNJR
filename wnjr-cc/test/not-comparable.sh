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
      echo "preprocesses chibicc's <stdarg.h>; wnjr-cc's va_list is a struct of its own (see wnjr-cc/include/stdarg.h)"
      return 0;;
    macro.c|pragma-once.c)
      # These include a path that only resolves with the working directory set to
      # chibicc's own tree, which chibicc's driver does and a guest cannot: its
      # working directory is the host's, fixed at the preopen.
      echo "includes a path that only resolves from chibicc's own tree, and a guest cannot change directory"
      return 0;;
  esac
  return 1
}
