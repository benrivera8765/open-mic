#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""check-macos-availability.py - the Python half of scripts/check-macos-availability.sh.

WORK_ORDER section 4.10 / trap 14, DECISIONS D-37: the owner's MacBook runs macOS 13.6.9
and CI runs macOS 15, so a binary that links an API introduced after macOS 13.0 without a
weak reference launches on CI and fails to load (dyld: "Symbol not found") on the MacBook.

Two sub-commands, standard library only (Python 3.8+, so the Command Line Tools' python3 on
the MacBook is enough):

  scan-sdk  --sdk DIR --out DB.json [--sdk-version V]
      Parses every header under DIR/System/Library/Frameworks (sub-frameworks included)
      and DIR/usr/include for C functions, C constants/variables, Objective-C classes and
      Objective-C methods/properties, and records the macOS version each one was introduced
      in, from API_AVAILABLE(macos(..)), __API_AVAILABLE, API_DEPRECATED(.., macos(..)),
      __OSX_AVAILABLE_STARTING(__MAC_..), __OSX_AVAILABLE(..), NS_AVAILABLE[_MAC](..),
      NS_CLASS_AVAILABLE[_MAC](..), CF_AVAILABLE[_MAC](..), *_AVAILABLE_STARTING(..),
      __attribute__((availability(macos, introduced=..))) and API_AVAILABLE_BEGIN/END
      regions. A declaration with no annotation counts as "always available".

  check     --db DB.json --work DIR [--max 13.0] [--json OUT.json]
      Reads DIR/files.tsv ("<id>\\t<path>" per Mach-O) and, per id, the saved output of
        <id>.nm        nm -m -u           (undefined symbols, with "weak" for weak imports)
        <id>.dyld      dyld_info -imports (fallback when nm gave nothing)
        <id>.selrefs   otool -v -s __DATA __objc_selrefs (+ __DATA_CONST)
        <id>.cstrings  otool -v -s __TEXT __cstring (+ __objc_methname, __objc_classname)
      and reports every reference to something introduced after --max:
        FAIL    a strong (non-weak) import of a C symbol or an Objective-C class
                (_OBJC_CLASS_$_X / _OBJC_METACLASS_$_X) newer than --max;
        REVIEW  a weak import of such a symbol (it must be behind a runtime check);
                a selector in __objc_selrefs that every SDK declaration marks as newer;
                a C string equal to such a class or selector name (objc2 and other
                runtimes look classes and selectors up by name; a lookup of a class that
                does not exist panics in objc2, so it must be behind a runtime check).
      Exit 0: no FAIL. 1: at least one FAIL. 2: bad input (no imports could be read).

Limits, stated plainly: this is a text parser, not a compiler. It does not evaluate #if,
so declarations from every branch are seen (a superset). Functions whose names are all
upper case with underscores (CC_SHA256 style) are mistaken for macros and not checked, and
so are all-caps selectors and properties (-URL, -UUID; selectors are REVIEW-only anyway).
Selectors are names only, so a selector shared with an older class is never reported.
It has been unit-tested on hand-written header snippets and fake tool output, and run on
the real Apple usr/include headers that Zig bundles; it has NOT run on a real macOS SDK
or a real Jukeoke.app (no Mac on the build PC; docs/MAC.md section 11).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys

# ---------------------------------------------------------------------------------------
# Versions
# ---------------------------------------------------------------------------------------
_VER = re.compile(r"^\s*(\d+)(?:[._](\d+))?(?:[._](\d+))?\s*$")


def parse_version(s):
    """'14.2', '14_2', '__MAC_14_2', '10.15.4' -> (14, 2, 0); None if not a version."""
    if s is None:
        return None
    s = s.strip()
    if s.startswith("__MAC_"):
        s = s[len("__MAC_"):]
    m = _VER.match(s)
    if not m:
        return None
    return tuple(int(g) if g else 0 for g in m.groups())


def vstr(v):
    if v is None:
        return "-"
    a, b, c = v
    return f"{a}.{b}" + (f".{c}" if c else "")


# ---------------------------------------------------------------------------------------
# Header text clean-up (offsets and line numbers are preserved)
# ---------------------------------------------------------------------------------------
_COMMENT_OR_STRING = re.compile(r"//[^\n]*|/\*.*?\*/|\"(?:\\.|[^\"\\\n])*\"|'(?:\\.|[^'\\\n])*'", re.S)


def _blank(s):
    return re.sub(r"[^\n]", " ", s)


def clean_header(text):
    """Comments blanked, string contents blanked, preprocessor lines blanked,
    `extern "C" {` removed. Same length and line breaks as the input (after CRLF -> LF)."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")

    def repl(m):
        s = m.group(0)
        if s[0] == "/":
            return _blank(s)
        return s[0] + " " * (len(s) - 2) + s[-1]

    text = _COMMENT_OR_STRING.sub(repl, text)
    out, cont = [], False
    for line in text.split("\n"):
        if cont or line.lstrip().startswith("#"):
            cont = line.rstrip().endswith("\\")
            out.append(" " * len(line))
        else:
            out.append(line)
    text = "\n".join(out)
    # `extern "C" {` (the string's contents are blank by now); its `}` is skipped later
    return re.sub(r"\bextern\s*\"[^\"\n]*\"\s*\{", lambda m: _blank(m.group(0)), text)


def match_paren(s, i):
    """s[i] == '(' -> index just after its matching ')' (len(s) if unbalanced)."""
    depth = 0
    for j in range(i, len(s)):
        c = s[j]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return j + 1
    return len(s)


# ---------------------------------------------------------------------------------------
# Annotations
# ---------------------------------------------------------------------------------------
_CALL = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(")
_MACOS_ARG = re.compile(r"\b(?:macos|macosx|macOS)\s*\(\s*([0-9][0-9._]*)")
_AVAIL_ATTR = re.compile(r"availability\s*\(\s*(?:macos|macosx)\s*,[^)]*?introduced\s*=\s*([0-9][0-9._]*)")
_NOT_MAC_MACRO = re.compile(r"IOS|IPHONE|TVOS|WATCHOS|VISIONOS|XROS|DRIVERKIT|MACCATALYST|BRIDGEOS", re.I)
_BARE_OLD = re.compile(r"\bAVAILABLE_MAC_OS_X_VERSION_(\d+)_(\d+)(?:_(\d+))?_AND_LATER\b")
_REGION_BEGIN = re.compile(r"\b(API_AVAILABLE_BEGIN|API_DEPRECATED_BEGIN|API_DEPRECATED_WITH_REPLACEMENT_BEGIN|API_UNAVAILABLE_BEGIN)\s*\(")
_REGION_END = re.compile(r"\b(API_AVAILABLE_END|API_DEPRECATED_END|API_UNAVAILABLE_END)\b")


def is_macro_name(name):
    """Apple's annotation/attribute/linkage macros: NS_SWIFT_NAME, API_AVAILABLE,
    __OSX_AVAILABLE_STARTING, CF_EXPORT, __attribute__ ... (all caps with an underscore,
    or a reserved leading underscore)."""
    if name in _ATTRIBUTE_WORDS:
        return True
    if name.startswith("__") or re.match(r"^_[A-Z]", name):
        return True
    return len(name) >= 2 and re.match(r"^[A-Z][A-Z0-9_]*$", name) is not None   # OF(), ZEXTERN, NS_SWIFT_NAME()


_ATTRIBUTE_WORDS = ("__attribute__", "__declspec", "_Pragma", "__asm", "__asm__", "asm", "__typeof__", "typeof")


def macro_version(name, args):
    """The macOS 'introduced' version an annotation macro call states, or None."""
    m = _MACOS_ARG.search(args)
    if m:
        return parse_version(m.group(1))
    m = _AVAIL_ATTR.search(args)
    if m:
        return parse_version(m.group(1))
    if "AVAILABLE" in name and "UNAVAILABLE" not in name and not _NOT_MAC_MACRO.search(name):
        first = args.split(",")[0].strip()
        if first in ("__MAC_NA", "NA", "__MAC_NA_"):
            return None
        return parse_version(first)
    return None


def annotations(text):
    """(max macOS version stated by any annotation in text or None, residual text with every
    annotation-like macro call and bare macro identifier removed, names of removed macros)."""
    versions, names, out, pos = [], [], [], 0
    for m in _CALL.finditer(text):
        if m.start() < pos:
            continue
        name = m.group(1)
        if not is_macro_name(name):
            continue
        if re.match(r"^__[a-z]", name) and name not in _ATTRIBUTE_WORDS:
            prev = text[:m.start()].rstrip()
            if prev and (prev[-1].isalnum() or prev[-1] in "_*") and "(" not in text[:m.start()]:
                continue   # a function with a reserved name in declarator position (__fabsf16), not a macro
        open_i = m.end() - 1
        close_i = match_paren(text, open_i)
        v = macro_version(name, text[open_i + 1:close_i - 1])
        if v:
            versions.append(v)
        names.append(name)
        out.append(text[pos:m.start()])
        out.append(" ")
        pos = close_i
    out.append(text[pos:])
    residual = "".join(out)
    for m in _BARE_OLD.finditer(residual):
        versions.append(tuple(int(g) if g else 0 for g in m.groups()))

    def bare(m):
        w = m.group(0)
        if is_macro_name(w):
            names.append(w)
            return " "
        return w

    # (a name followed by "(" that survived the loop above is a function: keep it)
    residual = re.sub(r"\b[A-Za-z_][A-Za-z0-9_]*\b(?!\s*\()", bare, residual)
    return (max(versions) if versions else None), residual, names


# ---------------------------------------------------------------------------------------
# Statements
# ---------------------------------------------------------------------------------------
_BOUNDARY_BEFORE = re.compile(r"^[ \t]*(?=[-+][ \t]*\(|@(?:property|end|optional|required|interface|protocol|implementation|class)\b)", re.M)
_BOUNDARY_AFTER = re.compile(r"@(?:end|optional|required)\b")
_OBJC_HEADER = re.compile(
    r"@(?:interface|protocol|implementation)\s+[A-Za-z_]\w*\s*(?:<[^<>;{}]*>)?\s*(?:\([^()]*\))?"
    r"\s*(?::\s*[A-Za-z_]\w*\s*(?:<[^<>;{}]*>)?)?\s*(?:<[^<>;{}]*>)?")
_TAIL_AFTER_BODY = re.compile(r"\b(struct|union|enum|typedef)\b|\b[A-Z_]+_(?:ENUM|OPTIONS)\s*\(")


def regions(clean):
    """API_AVAILABLE_BEGIN(...) ... API_AVAILABLE_END regions: (start, end, version or None),
    plus the text with those markers blanked."""
    found, stack, marks = [], [], []
    events = [(m.start(), "b", m) for m in _REGION_BEGIN.finditer(clean)] + \
             [(m.start(), "e", m) for m in _REGION_END.finditer(clean)]
    for start, kind, m in sorted(events, key=lambda e: e[0]):
        if kind == "b":
            close_i = match_paren(clean, m.end() - 1)
            stack.append((close_i, macro_version(m.group(1), clean[m.end():close_i - 1])))
            marks.append((m.start(), close_i))
        else:
            if stack:
                s, v = stack.pop()
                found.append((s, m.start(), v))
            marks.append((m.start(), m.end()))
    for s, v in stack:  # unterminated: until the end of the file
        found.append((s, len(clean), v))
    chars = list(clean)
    for a, b in marks:
        for i in range(a, b):
            if chars[i] != "\n":
                chars[i] = " "
    return found, "".join(chars)


def region_version(regs, offset):
    best = None
    for s, e, v in regs:
        if s <= offset < e and v is not None:
            if best is None or s > best[0]:
                best = (s, v)
    return best[1] if best else None


def split_statements(clean):
    """[(start_offset, text, terminator, after_body)]; terminator is ';', '{' or 'boundary'."""
    before = {m.end() for m in _BOUNDARY_BEFORE.finditer(clean)}
    after = {m.end() for m in _BOUNDARY_AFTER.finditer(clean)}
    after |= {m.end() for m in _OBJC_HEADER.finditer(clean)}
    out, n, i, start, after_body = [], len(clean), 0, 0, False

    def emit(end, term):
        nonlocal start, after_body
        txt = clean[start:end]
        if txt.strip():
            out.append((start, txt, term, after_body))
            after_body = False
        start = end

    while i < n:
        if i in before or i in after:
            emit(i, "boundary")
        c = clean[i]
        if c == ";":
            emit(i, ";")
            start = i + 1
            after_body = False
        elif c == "{":
            head = clean[start:i]
            emit(i, "{")
            depth, j = 0, i
            while j < n:
                if clean[j] == "{":
                    depth += 1
                elif clean[j] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            i = j
            start = j + 1
            after_body = bool(_TAIL_AFTER_BODY.search(head))
        elif c == "}":   # the closing brace of a removed `extern "C" {`
            emit(i, ";")
            start = i + 1
        i += 1
    emit(n, "boundary")
    return out


# ---------------------------------------------------------------------------------------
# Declarations
# ---------------------------------------------------------------------------------------
_KEYWORDS = {"void", "int", "char", "const", "unsigned", "signed", "long", "short", "float", "double",
             "struct", "union", "enum", "extern", "volatile", "register", "return", "sizeof", "id",
             "BOOL", "static", "inline", "restrict", "auto", "bool", "if", "while", "for", "switch",
             "typedef", "nonnull", "nullable", "null_unspecified", "null_resettable", "atomic",
             "nonatomic", "copy", "strong", "weak", "assign", "retain", "readonly", "readwrite",
             "class", "instancetype", "SEL", "IMP", "Class", "in", "out", "inout", "oneway", "bycopy",
             "byref"}
_SKIP_C = re.compile(r"\btypedef\b|\bstatic\b|\binline\b|__inline|_inline\b|INLINE\b|@")


def strip_parens(s):
    out, depth = [], 0
    for c in s:
        if c == "(":
            depth += 1
        elif c == ")":
            depth = max(0, depth - 1)
        elif depth == 0:
            out.append(c)
    return "".join(out)


def c_declaration(stmt, residual, macro_names):
    """('function'|'variable', name) for an external C declaration, else None."""
    if _SKIP_C.search(stmt):
        return None
    r = residual.strip()
    if not r or r.startswith(("-", "+")):
        return None
    m = re.search(r"\(\s*[*^]\s*([A-Za-z_]\w*)\s*\)\s*\(", r)  # function pointer / block variable
    if m:
        name = m.group(1)
        kind = "variable"
    else:
        name, kind = None, None
        depth = 0
        for i, c in enumerate(r):
            if c == "(":
                if depth == 0:
                    mm = re.search(r"([A-Za-z_]\w*)\s*$", r[:i])
                    if mm and mm.group(1) not in _KEYWORDS:
                        name, kind = mm.group(1), "function"
                    break
                depth += 1
            elif c == ")":
                depth -= 1
        if kind is None:
            linkage = re.search(r"\bextern\b", r) or any(("EXTERN" in n or "EXPORT" in n) for n in macro_names)
            if not linkage:
                return None
            body = re.sub(r"\[[^\]]*\]", " ", strip_parens(r))
            body = body.split("=")[0]
            words = [w for w in re.findall(r"[A-Za-z_]\w*", body) if w not in ("const", "volatile", "extern")]
            if not words:   # (an all-caps type such as BOOL was removed with the macros)
                return None
            name, kind = words[-1], "variable"
    if not name or name in _KEYWORDS:
        return None
    return kind, name


def method_selector(residual):
    s = residual.strip()
    if not s or s[0] not in "-+":
        return None
    s = s[1:].lstrip()
    if s.startswith("("):
        s = s[match_paren(s, 0):]
    s = strip_parens(s)
    s = s.split(",")[0]
    if ":" in s:
        parts = re.findall(r"([A-Za-z_]\w*)?\s*:", s)
        return "".join(p + ":" for p in parts) if parts else None
    m = re.search(r"[A-Za-z_]\w*", s)
    return m.group(0) if m else None


def property_selectors(stmt_residual, stmt_raw):
    m = re.search(r"@property\s*(\(([^()]*)\))?", stmt_raw)
    if not m:
        return []
    attrs = [a.strip() for a in (m.group(2) or "").split(",") if a.strip()]
    rest = stmt_residual[stmt_residual.find("@property") + len("@property"):] if "@property" in stmt_residual else stmt_residual
    if rest.lstrip().startswith("("):
        k = rest.find("(")
        rest = rest[match_paren(rest, k):]
    blk = re.search(r"\(\s*[*^]\s*([A-Za-z_]\w*)\s*\)", rest)
    if blk:
        name = blk.group(1)
    else:
        words = re.findall(r"[A-Za-z_]\w*", strip_parens(rest))
        words = [w for w in words if w not in ("const",)]
        if not words:
            return []
        name = words[-1]
    getter = name
    setter = "set" + name[:1].upper() + name[1:] + ":"
    readonly = False
    for a in attrs:
        if a == "readonly":
            readonly = True
        elif a.startswith("getter"):
            getter = a.split("=", 1)[1].strip()
        elif a.startswith("setter"):
            setter = a.split("=", 1)[1].strip()
    return [getter] if readonly else [getter, setter]


def parse_header(text, where="?"):
    """All declarations of one header: list of dicts
    {kind: function|variable|class|selector, name, v: (maj,min,patch) or None, where}."""
    clean = clean_header(text)
    regs, clean = regions(clean)
    line_starts = [0] + [i + 1 for i, c in enumerate(clean) if c == "\n"]

    def line_of(off):
        lo, hi = 0, len(line_starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if line_starts[mid] <= off:
                lo = mid
            else:
                hi = mid - 1
        return lo + 1

    decls = []
    container = None          # dict(kind, name, v)
    carry_text, carry_off = "", None
    for off, txt, term, after_body in split_statements(clean):
        next_char = clean[off + len(txt):].lstrip()[:1]
        # A region applies where the declaration ENDS: the statement's text may start before
        # API_AVAILABLE_BEGIN (blank lines, NS_ASSUME_NONNULL_BEGIN) or before API_AVAILABLE_END
        # (the text after the region's last ';'), so its start offset says nothing.
        end_off = off + len(txt.rstrip()) - 1
        if carry_text:
            txt = carry_text + " " + txt
            off = carry_off
            carry_text, carry_off = "", None
        v_own, residual, names = annotations(txt)
        r = residual.strip()
        if not r:
            if term == "boundary":   # annotations on their own line(s) belong to what follows
                carry_text, carry_off = txt, off
            continue
        loc = f"{where}:{line_of(off + len(txt) - len(txt.lstrip()))}"
        v_region = region_version(regs, end_off)
        if r.startswith("@end"):
            container = None
            continue
        mh = None
        if r.startswith(("@interface", "@protocol", "@implementation")):
            # Read the name from the statement itself, not the residual: an all-caps class
            # name (NSURL style) looks like a macro and is gone from the residual.
            mh = re.search(r"@(interface|protocol|implementation)\s+([A-Za-z_]\w*)\s*(?:<[^<>]*>)?\s*(\([^()]*\))?", txt)
        if mh:
            kind, name, category = mh.group(1), mh.group(2), mh.group(3)
            if kind == "protocol" and (term == ";" or next_char in (";", ",")):   # forward declaration
                continue
            v = v_own or v_region
            container = {"kind": kind if not category else "category", "name": name, "v": v}
            if kind == "interface" and not category:
                decls.append({"kind": "class", "name": name, "v": v, "where": loc})
            continue
        if r.startswith(("@class", "@optional", "@required", "@synthesize", "@dynamic", "@compatibility_alias")):
            continue
        if container is not None and container["kind"] != "implementation" and (r.startswith(("-", "+", "@property"))):
            base = container["v"] if container["v"] is not None else v_region
            v = v_own if v_own is not None else base
            if v is not None and base is not None and v < base:
                v = base
            sels = property_selectors(residual, txt) if r.startswith("@property") else [method_selector(residual)]
            for sel in sels:
                if sel:
                    decls.append({"kind": "selector", "name": sel, "v": v, "where": loc})
            continue
        if after_body:
            continue
        cd = c_declaration(txt, residual, names)
        if cd:
            decls.append({"kind": cd[0], "name": cd[1], "v": v_own or v_region, "where": loc})
    return decls


# ---------------------------------------------------------------------------------------
# SDK scan -> database
# ---------------------------------------------------------------------------------------
def header_files(sdk):
    roots = [os.path.join(sdk, "System", "Library", "Frameworks"), os.path.join(sdk, "usr", "include")]
    for root in roots:
        if not os.path.isdir(root):
            continue
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames.sort()
            for fn in sorted(filenames):
                if fn.endswith(".h"):
                    p = os.path.join(dirpath, fn)
                    if root.endswith("Frameworks") and os.sep + "Headers" not in p and "/Headers" not in p:
                        continue   # Modules/, Versions/ symlink trees without headers
                    yield p, os.path.relpath(p, sdk).replace(os.sep, "/")


def build_db(files):
    """files: iterable of (text, where). Returns the database dict.
    symbols: Mach-O symbol name (leading '_') -> {v, kind, where}; classes: name -> {v, where};
    selectors: name -> {v, where}. Each keeps the OLDEST declaration; one declaration without
    an annotation makes the entry 'always available' and it is dropped."""
    sym, cls, sel = {}, {}, {}
    headers = annotated = 0

    def keep(table, key, v, extra):
        cur = table.get(key)
        vv = v if v is not None else (0, 0, 0)
        if cur is None or vv < cur["_v"]:
            table[key] = dict(extra, _v=vv)

    for text, where in files:
        headers += 1
        for d in parse_header(text, where):
            if d["v"] is not None:
                annotated += 1
            if d["kind"] in ("function", "variable"):
                keep(sym, "_" + d["name"], d["v"], {"kind": d["kind"], "where": d["where"]})
            elif d["kind"] == "class":
                keep(cls, d["name"], d["v"], {"where": d["where"]})
            elif d["kind"] == "selector":
                keep(sel, d["name"], d["v"], {"where": d["where"]})

    def finish(table):
        out = {}
        for k, e in table.items():
            if e["_v"] == (0, 0, 0):
                continue
            e = dict(e)
            e["v"] = vstr(e.pop("_v"))
            out[k] = e
        return dict(sorted(out.items()))

    return {"schema": "jukeoke-macos-availability/1", "headers": headers, "annotated_declarations": annotated,
            "symbols": finish(sym), "classes": finish(cls), "selectors": finish(sel)}


def newer_count(db, max_v):
    n = 0
    for table in ("symbols", "classes", "selectors"):
        n += sum(1 for e in db[table].values() if parse_version(e["v"]) > max_v)
    return n


DEFAULT_CANARIES = "symbol:_AudioHardwareCreateProcessTap=14.2,class:SCContentSharingPicker=14.0"


def check_canaries(db, spec):
    """Known declarations the parser must find, so a parser that finds nothing cannot pass.
    spec: 'symbol:_X=14.2,class:Y=14.0,selector:z:=14.0'. Returns a list of problems."""
    problems = []
    for item in [s.strip() for s in spec.split(",") if s.strip()]:
        try:
            kind, rest = item.split(":", 1)
            name, want = rest.rsplit("=", 1)
        except ValueError:
            problems.append(f"bad canary '{item}'")
            continue
        table = {"symbol": "symbols", "class": "classes", "selector": "selectors"}.get(kind)
        e = db.get(table, {}).get(name) if table else None
        if e is None:
            problems.append(f"canary {kind} {name} (macOS {want}) was not found in the SDK headers")
        elif parse_version(e["v"]) != parse_version(want):
            problems.append(f"canary {kind} {name}: parsed macOS {e['v']}, expected {want} ({e['where']})")
    return problems


# ---------------------------------------------------------------------------------------
# Tool output parsers
# ---------------------------------------------------------------------------------------
_NM_LINE = re.compile(r"\(undefined(?:\s*\[[^\]]*\])?\)\s+(weak\s+)?(?:private\s+)?(?:external|non-external)\s+(\S+)(?:\s+\(from\s+([^)]+)\))?")


def parse_nm_m(text):
    """`nm -m -u` (Apple nm): [(symbol, weak, library)]."""
    out = []
    for line in text.splitlines():
        m = _NM_LINE.search(line)
        if m:
            out.append((m.group(2), bool(m.group(1)), m.group(3)))
    return out


_DYLD_LINE = re.compile(r"(?:^|\s)(_[A-Za-z0-9_$.]+)\b(.*)$")


def parse_dyld_info_imports(text):
    """`dyld_info -imports`: [(symbol, weak, library)]. Read liberally: any line naming a
    _symbol; 'weak' anywhere on the line marks a weak import; '(from X)' names the library."""
    out = []
    for line in text.splitlines():
        if line.rstrip().endswith(":") or "-imports" in line:
            continue
        m = _DYLD_LINE.search(line)
        if not m:
            continue
        rest = m.group(2)
        lib = re.search(r"\(from\s+([^)]+)\)", rest)
        # 'weak' outside the symbol's own name (_objc_storeWeak is not a weak import)
        weak = "weak" in (line[:m.start(1)] + rest).lower()
        out.append((m.group(1), weak, lib.group(1) if lib else None))
    return out


def parse_otool_selrefs(text):
    """`otool -v -s __DATA __objc_selrefs`: selector names."""
    return sorted({m.group(1) for m in re.finditer(r"__objc_methname:(\S+)", text)})


def parse_otool_cstrings(text):
    """`otool -v -s __TEXT __cstring`: the strings ('<address>  <string>' lines)."""
    out = []
    for line in text.splitlines():
        m = re.match(r"^\s*(?:0x)?[0-9a-fA-F]{6,}\s+(.*)$", line)
        if m:
            out.append(m.group(1).strip())
    return out


# ---------------------------------------------------------------------------------------
# Matcher
# ---------------------------------------------------------------------------------------
def match_file(db, max_v, imports, selrefs, cstrings):
    """Findings for one Mach-O: list of dicts {verdict, what, name, v, ref, where}."""
    out = []
    for sym, weak, lib in imports:
        what, e = None, None
        for prefix, label in (("_OBJC_CLASS_$_", "class"), ("_OBJC_METACLASS_$_", "metaclass")):
            if sym.startswith(prefix):
                what, e = label, db["classes"].get(sym[len(prefix):])
                break
        else:
            what, e = "symbol", db["symbols"].get(sym)
        if e is None or not parse_version(e["v"]) > max_v:
            continue
        out.append({"verdict": "REVIEW" if weak else "FAIL", "what": what, "name": sym, "v": e["v"],
                    "ref": ("weak import" if weak else "STRONG import") + (f" from {lib}" if lib else ""),
                    "where": e["where"]})
    for s in selrefs:
        e = db["selectors"].get(s)
        if e and parse_version(e["v"]) > max_v:
            out.append({"verdict": "REVIEW", "what": "selector", "name": s, "v": e["v"],
                        "ref": "__objc_selrefs", "where": e["where"]})
    seen = set()
    for s in cstrings:
        if s in seen:
            continue
        seen.add(s)
        for table, what in (("classes", "class name string"), ("selectors", "selector name string")):
            e = db[table].get(s)
            if e and parse_version(e["v"]) > max_v and (what != "selector name string" or ":" in s or len(s) >= 8):
                out.append({"verdict": "REVIEW", "what": what, "name": s, "v": e["v"],
                            "ref": "__cstring (looked up by name at run time?)", "where": e["where"]})
    order = {"FAIL": 0, "REVIEW": 1}
    return sorted(out, key=lambda f: (order[f["verdict"]], f["what"], f["name"]))


def read_opt(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return ""


def cmd_check(args):
    max_v = parse_version(args.max)
    with open(args.db, encoding="utf-8") as f:
        db = json.load(f)
    rows = []
    with open(os.path.join(args.work, "files.tsv"), encoding="utf-8") as f:
        for line in f:
            if line.strip():
                fid, path = line.rstrip("\n").split("\t", 1)
                rows.append((fid, path))
    if not rows:
        print("check-macos-availability: no Mach-O files to check", file=sys.stderr)
        return 2
    report, total_fail, total_review, unreadable = [], 0, 0, []
    for fid, path in rows:
        base = os.path.join(args.work, fid)
        imports = parse_nm_m(read_opt(base + ".nm"))
        source = "nm -m"
        if not imports:
            imports = parse_dyld_info_imports(read_opt(base + ".dyld"))
            source = "dyld_info -imports"
        if not imports:
            unreadable.append(path)
            continue
        found = match_file(db, max_v, imports, parse_otool_selrefs(read_opt(base + ".selrefs")),
                           parse_otool_cstrings(read_opt(base + ".cstrings")))
        nf = sum(1 for x in found if x["verdict"] == "FAIL")
        total_fail += nf
        total_review += len(found) - nf
        report.append({"file": path, "imports": len(imports), "weak_imports": sum(1 for i in imports if i[1]),
                       "read_with": source, "findings": found})
    print(f"macOS API availability: allowed = introduced in macOS {vstr(max_v)} or earlier; "
          f"SDK database: {db.get('headers', '?')} headers, {len(db['symbols'])} symbols, "
          f"{len(db['classes'])} classes, {len(db['selectors'])} selectors with an annotation")
    for r in report:
        print(f"\n{r['file']}: {r['imports']} imported symbols ({r['weak_imports']} weak, read with {r['read_with']})")
        if not r["findings"]:
            print("    ok: nothing newer than macOS " + vstr(max_v))
        for x in r["findings"]:
            print(f"    {x['verdict']:6} {x['what']:20} {x['name']}  macOS {x['v']}  {x['ref']}  [{x['where']}]")
    for p in unreadable:
        print(f"\n{p}: ERROR: no imported symbols could be read (nm -m and dyld_info output empty)")
    print(f"\n{len(report)} file(s) checked: {total_fail} FAIL (strong reference to an API newer than macOS "
          f"{vstr(max_v)}), {total_review} REVIEW (weak references, selectors, names looked up at run time: "
          f"each must sit behind a runtime availability check); {len(unreadable)} unreadable")
    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump({"max": vstr(max_v), "files": report, "unreadable": unreadable,
                       "fail": total_fail, "review": total_review}, f, indent=1)
    if total_fail:
        return 1
    return 2 if unreadable else 0


def cmd_scan(args):
    files = []
    for path, rel in header_files(args.sdk):
        with open(path, encoding="utf-8", errors="replace") as f:
            files.append((f.read(), rel))
    if not files:
        print(f"check-macos-availability: no headers under {args.sdk}/System/Library/Frameworks or usr/include", file=sys.stderr)
        return 2
    db = build_db(files)
    db["sdk"] = os.path.basename(os.path.normpath(args.sdk))
    db["sdk_version"] = args.sdk_version or ""
    max_v = parse_version(args.max)
    n_new = newer_count(db, max_v)
    print(f"scanned {db['headers']} headers: {db['annotated_declarations']} annotated declarations; "
          f"{len(db['symbols'])} C symbols, {len(db['classes'])} classes, {len(db['selectors'])} selectors "
          f"with a macOS version; {n_new} newer than macOS {vstr(max_v)}")
    problems = []
    if n_new < args.min_newer:
        problems.append(f"only {n_new} declarations newer than macOS {vstr(max_v)} were found (expected at least "
                        f"{args.min_newer}): the header parser is not reading this SDK")
    sdk_v = parse_version(args.sdk_version) if args.sdk_version else None
    if args.canaries and (sdk_v is None or sdk_v >= (14, 2, 0)):
        problems += check_canaries(db, args.canaries)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(db, f, indent=0, sort_keys=True)
    for p in problems:
        print("check-macos-availability: ERROR: " + p, file=sys.stderr)
    return 2 if problems else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("scan-sdk")
    s.add_argument("--sdk", required=True)
    s.add_argument("--out", required=True)
    s.add_argument("--sdk-version", default="")
    s.add_argument("--max", default="13.0")
    s.add_argument("--min-newer", type=int, default=200)
    s.add_argument("--canaries", default=os.environ.get("JK_AVAIL_CANARIES", DEFAULT_CANARIES))
    c = sub.add_parser("check")
    c.add_argument("--db", required=True)
    c.add_argument("--work", required=True)
    c.add_argument("--max", default="13.0")
    c.add_argument("--json", default="")
    args = ap.parse_args(argv)
    return cmd_scan(args) if args.cmd == "scan-sdk" else cmd_check(args)


if __name__ == "__main__":
    sys.exit(main())
