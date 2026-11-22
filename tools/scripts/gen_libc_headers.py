#!/usr/bin/env python3
"""Generate njvm-cc/libc_headers.c, the table of welded headers.

A welded header is a libc header this compiler carries already preprocessed:
its declarations as plain text, plus the macros it leaves behind. njvm-cc reads
that table instead of opening the file, expanding macros and walking include
paths, which is most of what compiling the libc costs: preprocessing is 96% of
it (2.39s of 2.47s over the libc 263 units, arm64 M5).

    python3 tools/scripts/gen_libc_headers.py         write the table
    python3 tools/scripts/gen_libc_headers.py --check is the table current
    python3 tools/scripts/gen_libc_headers.py -v      one line per header

--check is what the gate runs. It regenerates the table in memory and compares
it to the file in the tree, so a header that changed without the table being
regenerated fails the gate instead of quietly going stale.

The table is produced by njvm-cc own preprocessor (wcc -E), not by a second
implementation, so it cannot disagree with the compiler that reads it. The
headers are found through the compiler own search paths (wcc
--dump-include-paths), in the order it would search them, so a name is welded
from the directory that would have answered it.

What makes a name safe to weld is decided in preprocess.c, not here: the table
answers an include only while the unit has not defined a macro beginning with
underscore. Those are how a program asks a header for a different
configuration (_GNU_SOURCE, _POSIX_C_SOURCE, _FILE_OFFSET_BITS), and the welded
text was expanded under the default one. Any such definition, or a -D on the
command line, and the unit reads the headers from disk again.
"""

import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
CC_DIR = os.path.join(ROOT, "njvm-cc")
WCC = os.path.join(CC_DIR, "wcc")
OUT = os.path.join(CC_DIR, "libc_headers.c")

# These two change from run to run. They are predefined, so they are in the
# baseline too, but comparing by name alone would let a header that mentions
# them look like it defined them.
VOLATILE = {"__DATE__", "__TIME__"}


def run(args):
    p = subprocess.run(args, capture_output=True)
    return p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")


def fail(msg):
    sys.stderr.write("gen_libc_headers: " + msg + chr(10))
    sys.exit(1)


def include_roots():
    code, out, err = run([WCC, "--dump-include-paths"])
    if code != 0:
        fail("wcc --dump-include-paths failed: " + err.strip())
    roots = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        # Only directories of this tree. /usr/include and /usr/local/include are
        # on the search path too but belong to the host: welding them would bake
        # a machine headers into the compiler.
        path = os.path.abspath(line)
        if path == ROOT or path.startswith(ROOT + os.sep):
            roots.append(path)
    return roots


def used_names(roots):
    """The names a libc build actually includes, plus this compiler own headers.

    Welding every file under the search path would put the internal headers in
    the table too (bits/*.h, __struct_*.h): a program does not name those, and a
    header that folds one already carries its text. A name outside this set is
    not an error at run time, only a header read from disk, so the set can be
    this narrow without being wrong.
    """
    names = set()

    libc_root = os.path.join(ROOT, "njvm-cc-libc")
    units = []
    for dirpath, dirnames, filenames in os.walk(libc_root):
        dirnames.sort()
        for fn in sorted(filenames):
            if fn.endswith(".c"):
                units.append(os.path.join(dirpath, fn))

    for unit in units:
        # --no-weld: what the unit depends on is a fact about the source, not
        # about what this compiler happens to carry. Asking with the table on
        # would report only the headers the table did not answer, and the set
        # would shrink every time it was regenerated.
        code, deps, err = run([WCC, "--no-weld", "--deps", unit])
        if code != 0:
            continue
        for line in deps.splitlines():
            n = name_of(line.strip(), roots)
            if n:
                names.add(n)

    # This compiler own headers: they are first on the search path and are what
    # the front end needs to agree with itself.
    own = os.path.join(CC_DIR, "include")
    for dirpath, dirnames, filenames in os.walk(own):
        for fn in filenames:
            if fn.endswith(".h"):
                names.add(os.path.relpath(os.path.join(dirpath, fn), own))

    return sorted(names)


def header_names(roots):
    """Every name an include can ask for, first directory wins."""
    seen = {}
    for root in roots:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames.sort()
            for fn in sorted(filenames):
                if not fn.endswith(".h"):
                    continue
                name = os.path.relpath(os.path.join(dirpath, fn), root)
                if name not in seen:
                    seen[name] = os.path.join(dirpath, fn)
    return seen


def name_of(path, roots):
    """The include name a resolved path answers to, or None."""
    path = os.path.abspath(path)
    for root in roots:
        if path.startswith(root + os.sep):
            return os.path.relpath(path, root)
    return None


def first_error(stderr):
    for line in stderr.splitlines():
        line = line.strip()
        if line:
            return line
    return "unknown failure"


def expand(name, src, with_features=True):
    """One header expanded: a tree of files, and the macro lines it leaves.

    features.h first, because it decides how every other libc header comes out:
    it sets __wasilibc_* and the _BSD_SOURCE family, and a header included
    without it in effect declares less than the same header included after it.
    Every real unit has reached features.h through its first include, so the
    probe does too. with_features=False answers what the header looks like with
    no such state; a header whose two answers differ is one the table cannot
    hold, see the state check below.
    """
    with open(src, "w") as f:
        if with_features:
            f.write("#include <features.h>" + chr(10))
        f.write("#include <" + name + ">" + chr(10))

    # --no-weld throughout: the table this run produces must describe the
    # headers, not the table it is replacing. With the table on, the header it
    # already answers for would come back as its own dependencies' absence, and
    # a second run would produce a different table from the first.
    # --weld-tree: the output carries a marker where every included file begins
    # and ends, so what a header adds on its own can be read off rather than
    # guessed at.
    code, text, err = run([WCC, "--no-weld", "--emit-text", "--weld-tree", src])
    if code != 0:
        return None, first_error(err)

    code, mac, err = run([WCC, "--no-weld", "--dump-macros", src])
    if code != 0:
        return None, first_error(err)

    return (split_tree(text), mac), None


BEGIN = chr(1)
END = chr(2)


def flatten(frame, welded, roots, skip_name):
    """A file's own tokens, with the files it includes kept only where they are
    not entries of their own.

    A welded child is spliced from its own entry, so it is left out here; a child
    that is not welded would be lost if it were left out, so it stays.
    """
    out = list(frame["own"])
    for child in frame["children"]:
        n = name_of(child["name"], roots)
        if n and n != skip_name and n in welded:
            continue
        out.append(flatten(child, welded, roots, skip_name))
    return "".join(out)


def split_tree(text):
    """The preprocessed output as a tree of files.

    Everything between a file marker and the markers of the files it includes is
    that file's own tokens, which is exactly what a welded entry stores. The
    boundary is the compiler's, not one guessed from the text.
    """
    root = {"name": "", "own": [], "children": []}
    stack = [root]
    i = 0
    while i < len(text):
        ch = text[i]
        if ch == BEGIN:
            j = text.index(ch, i + 1)
            stack.append({"name": text[i + 1:j].strip(), "own": [],
                          "children": []})
            i = j + 1
        elif ch == END:
            j = text.index(ch, i + 1)
            i = j + 1
            if len(stack) > 1:
                done = stack.pop()
                stack[-1]["children"].append(done)
        else:
            stack[-1]["own"].append(ch)
            i += 1
    return root


BS = chr(92)      # backslash
DQ = chr(34)      # double quote


def quote_c(text):
    """A C string literal for text, split into pieces of at most 200 characters.

    An octal escape is at most three digits and a digit right after one is not
    part of it, so a piece always ends at an escape: the next piece then starts
    with the character that would otherwise have been swallowed.
    """
    pieces = []
    cur = []
    n = 0
    for ch in text.encode("utf-8"):
        if ch == 10:
            e = BS + "n"
            esc = True
        elif ch == 9:
            e = BS + "t"
            esc = True
        elif ch == 34:
            e = BS + DQ
            esc = True
        elif ch == 92:
            e = BS + BS
            esc = True
        elif 0x20 <= ch < 0x7F:
            e = chr(ch)
            esc = False
        else:
            e = BS + "%03o" % ch
            esc = True
        cur.append(e)
        n += 1
        # Only length matters: an octal escape is at most three digits, so a
        # digit after one is a character of its own and not part of it.
        if n >= 200:
            pieces.append("".join(cur))
            cur = []
            n = 0
    if cur or not pieces:
        pieces.append("".join(cur))
    sep = chr(10) + "    "
    return sep.join(DQ + p + DQ for p in pieces)



def macro_name(line):
    """The name a #define line defines."""
    rest = line[len("#define "):]
    for i, ch in enumerate(rest):
        if ch == "(" or ch == " ":
            return rest[:i]
    return rest


def main():
    verbose = "-v" in sys.argv[1:]
    check = "--check" in sys.argv[1:]

    code, out, err = run(["make", "-s", "-C", CC_DIR, "wcc"])
    if code != 0:
        fail("cannot build wcc: " + err.strip())

    roots = include_roots()
    names = used_names(roots)

    work = tempfile.mkdtemp(prefix="weld-")
    src = os.path.join(work, "probe.c")

    # The macros every unit starts with, so that what a header leaves behind can
    # be told apart from what was already there.
    empty = os.path.join(work, "empty.c")
    with open(empty, "w") as f:
        f.write("")
    code, base_text, err = run([WCC, "--dump-macros", empty])
    if code != 0:
        fail("cannot dump the predefined macros: " + err.strip())
    baseline = set()
    for line in base_text.splitlines():
        if line.startswith("#define "):
            baseline.add(macro_name(line))

    skipped = []
    frames = {}
    deps_of = {}
    macs = {}
    order = []
    for name in sorted(names):
        got, why = expand(name, src)
        if got is None:
            skipped.append((name, why))
            continue
        tree, mac = got

        # The probe includes features.h and then this header, so the header is
        # the child of the root whose path names it. Its own tokens are what is
        # left once the files it includes are taken out, which the markers have
        # already separated.
        frame = None
        for child in tree["children"]:
            if name_of(child["name"], roots) == name:
                frame = child
                break
        if frame is None:
            skipped.append((name, "the probe did not reach it"))
            continue

        # A header whose own tokens depend on what was included before it cannot
        # be one welded entry: musl's bits/alltypes.h is nothing but conditions
        # on __NEED_*, and it answers differently for every includer. Such a
        # header stays inline in whichever header reaches it, where the text is
        # the one that includer asked for.
        bare, why = expand(name, src, with_features=False)
        if bare is None:
            skipped.append((name, why))
            continue
        bare_frame = None
        for child in bare[0]["children"]:
            if name_of(child["name"], roots) == name:
                bare_frame = child
                break
        if (bare_frame is None
                or flatten(bare_frame, set(), roots, name)
                != flatten(frame, set(), roots, name)):
            skipped.append((name, "its tokens depend on what included it"))
            continue

        # What the probe put in front of this header counts as its dependency:
        # features.h sets the macros the header was expanded under, and a header
        # that uses a type features.h brought in needs it just as much as one it
        # includes itself.
        d = []
        for child in tree["children"]:
            n = name_of(child["name"], roots)
            if n and n != name and n not in d:
                d.append(n)
        for child in frame["children"]:
            n = name_of(child["name"], roots)
            if n and n != name and n not in d:
                d.append(n)

        frames[name] = frame
        deps_of[name] = d
        macs[name] = mac
        order.append(name)

    welded = set(order)
    entries = []
    total_text = 0
    total_mac = 0
    for name in order:
        own = flatten(frames[name], welded, roots, name)

        mac_lines = []
        for line in macs[name].splitlines():
            if not line.startswith("#define "):
                continue
            mname = macro_name(line)
            if mname in baseline or mname in VOLATILE:
                continue
            mac_lines.append(line)
        mac_lines.sort()

        deps = [d for d in deps_of[name] if d in welded]
        total_text += len(own)
        total_mac += sum(len(l) + 1 for l in mac_lines)
        entries.append((name, own, chr(10).join(mac_lines), deps))
        if verbose:
            print("[weld] %-40s own=%6d macros=%3d deps=%2d"
                  % (name, len(own), len(mac_lines), len(deps)))

    L = []
    add = L.append
    add("// Generated by tools/scripts/gen_libc_headers.py: do not edit.")
    add("//")
    add("// One entry per libc header this compiler carries already")
    add("// preprocessed: what the header declares, the macros it leaves")
    add("// behind, and the headers whose text is already part of it. The")
    add("// code that reads it is preprocess.c; the types are in weld.h.")
    add("//")
    add("// Regenerate once a libc header changes:")
    add("//     python3 tools/scripts/gen_libc_headers.py")
    add("// Check that the table is current, which is what the gate does:")
    add("//     python3 tools/scripts/gen_libc_headers.py --check")
    add("")
    add(chr(35) + "include " + DQ + "weld.h" + DQ)
    add("")

    for i, (name, text, mac, deps) in enumerate(entries):
        add("// " + name)
        if deps:
            add("static const char *deps_%d[] = {%s};"
                % (i, ", ".join(DQ + d + DQ for d in deps)))
        add("static const char text_%d[] = %s;" % (i, quote_c(text)))
        if mac:
            add("static const char macros_%d[] = %s;" % (i, quote_c(mac + chr(10))))
        add("")

    add("const WeldHeader weld_headers[] = {")
    for i, (name, text, mac, deps) in enumerate(entries):
        add("  {" + DQ + name + DQ + ", text_%d, %s, %s, %d},"
            % (i, "macros_%d" % i if mac else "0",
               "deps_%d" % i if deps else "0", len(deps)))
    add("};")
    add("")
    add("const int weld_headers_len =")
    add("    sizeof(weld_headers) / sizeof(*weld_headers);")
    add("")

    result = chr(10).join(L)

    print("headers: %d welded, %d skipped" % (len(entries), len(skipped)))
    print("size:    %d bytes of declarations, %d bytes of macros"
          % (total_text, total_mac))
    for name, why in skipped:
        print("[skip] %s: %s" % (name, why))

    rel = os.path.relpath(OUT, ROOT)
    if check:
        try:
            with open(OUT) as f:
                current = f.read()
        except FileNotFoundError:
            current = None
        if current == result:
            print(rel + " is current")
            return 0
        print(rel + " is STALE: run python3 tools/scripts/gen_libc_headers.py")
        return 1

    with open(OUT, "w") as f:
        f.write(result)
    print("wrote " + rel)
    return 0


if __name__ == "__main__":
    sys.exit(main())

