#!/usr/bin/env python3
# license:BSD-3-Clause
"""Check the UI panel translations (src/ui/texts*.h + src/ui/lang.h).

Every language file must define exactly the field set of ui_texts
(missing/extra entries fail), and every *_fmt must carry the same printf
sequences in every language (a translator typo like %s -> %d would crash).

Call sites spell UI_TEXT(id, "English default"); the default must equal
the en entry (drift fails), and an id missing from the struct fails with
paste-ready struct/ja/en lines.

Usage: python3 tools/check_texts.py
Exit 0 when everything matches, 1 otherwise.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANG_H = ROOT / "src/ui/lang.h"
TEXTS_H = ROOT / "src/ui/texts.h"

FIELD_RE = re.compile(r"const\s+char\s*\*\s*(\w+)\s*;")
ENTRY_RE = re.compile(r"\.(\w+)\s*=\s*((?:\"(?:[^\"\\]|\\.)*\"\s*)+)[,;]", re.S)
LITERAL_RE = re.compile(r"\"((?:[^\"\\]|\\.)*)\"")
SPEC_RE = re.compile(
    r"%(?:%%|[-+0 #'0-9]*(?:\.\d+)?[hljztL]*[diuoxXfFeEgGaAcspn])"
)


def fail(msg, errors):
    errors.append(msg)
    print(f"FAIL: {msg}")


def lang_codes():
    lines = LANG_H.read_text(encoding="utf-8").splitlines()
    body = []
    for i, line in enumerate(lines):
        if line.strip().startswith("#define UI_LANG_LIST"):
            frag = line.split("UI_LANG_LIST(X)", 1)[1]
            body.append(frag)
            j = i
            while body[-1].rstrip().endswith("\\") and j + 1 < len(lines):
                j += 1
                body.append(lines[j])
            break
    if not body:
        print("FAIL: UI_LANG_LIST not found in src/ui/lang.h")
        sys.exit(1)
    codes = re.findall(r"X\(\s*(\w+)\s*,", "\n".join(body))
    if not codes:
        print("FAIL: UI_LANG_LIST has no X(code, ...) entries in src/ui/lang.h")
        sys.exit(1)
    return codes


def struct_fields():
    text = TEXTS_H.read_text(encoding="utf-8")
    m = re.search(r"struct\s+ui_texts\s*\{(.*?)\};", text, re.S)
    if not m:
        print("FAIL: struct ui_texts not found in src/ui/texts.h")
        sys.exit(1)
    return FIELD_RE.findall(m.group(1))


def printf_specs(joined):
    # %% is an escaped percent, not a conversion.
    return [s for s in SPEC_RE.findall(joined) if s != "%%"]


def unescape(joined):
    out = []
    i = 0
    simple = {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"', "'": "'",
              "0": "\0"}
    while i < len(joined):
        c = joined[i]
        if c == "\\" and i + 1 < len(joined) and joined[i + 1] in simple:
            out.append(simple[joined[i + 1]])
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def ui_text_sites():
    # id -> [(path, lineno, default joined raw)]. String-aware, so parens
    # and commas inside the default literal do not confuse it.
    sites = {}
    for path in sorted((ROOT / "src").rglob("*")):
        if path.suffix not in (".h", ".cpp", ".mm"):
            continue
        if path.name.startswith("texts_") or path.name == "texts.h":
            continue
        text = path.read_text(encoding="utf-8")
        i = 0
        while True:
            j = text.find("UI_TEXT", i)
            if j < 0:
                break
            i = j + len("UI_TEXT")
            k = i
            while k < len(text) and text[k] in " \t\r\n":
                k += 1
            if k >= len(text) or text[k] != "(":
                continue
            depth, in_str, esc = 0, False, False
            cur, parts = "", []
            p = k
            while p < len(text):
                c = text[p]
                if in_str:
                    cur += c
                    if esc:
                        esc = False
                    elif c == "\\":
                        esc = True
                    elif c == '"':
                        in_str = False
                elif c == '"':
                    in_str = True
                    cur += c
                elif c == '(':
                    if depth > 0:
                        cur += c
                    depth += 1
                elif c == ')':
                    depth -= 1
                    if depth == 0:
                        parts.append(cur)
                        p += 1
                        break
                    cur += c
                elif c == ',' and depth == 1:
                    parts.append(cur)
                    cur = ""
                else:
                    cur += c
                p += 1
            else:
                continue
            i = p
            if len(parts) < 2:
                continue
            name = parts[0].strip()
            default = "".join(LITERAL_RE.findall(parts[1]))
            lineno = text.count("\n", 0, j) + 1
            sites.setdefault(name, []).append(
                (str(path.relative_to(ROOT)), lineno, default))
    return sites


def main():
    errors = []
    codes = lang_codes()
    fields = struct_fields()
    print(f"languages: {', '.join(codes)}")
    print(f"fields ({len(fields)}): {', '.join(fields)}")

    if not codes:
        fail("no language codes in LANG_CODES", errors)
    if codes[0] != "ja":
        fail(f"first LANG_CODES entry should be ja (fallback), got {codes[0]}", errors)

    tables = {}
    for code in codes:
        path = ROOT / f"src/ui/texts_{code}.h"
        if not path.exists():
            fail(f"src/ui/texts_{code}.h missing for LANG_CODES entry '{code}'", errors)
            continue
        text = path.read_text(encoding="utf-8")
        seen = []
        values = {}
        dupes = set()
        for m in ENTRY_RE.finditer(text):
            name = m.group(1)
            if name in values:
                dupes.add(name)
            seen.append(name)
            values[name] = "".join(LITERAL_RE.findall(m.group(2)))
        for d in sorted(dupes):
            fail(f"texts_{code}.h: duplicate .{d}", errors)
        missing = [f for f in fields if f not in values]
        extra = [n for n in seen if n not in fields]
        for f in missing:
            fail(f"texts_{code}.h: missing .{f}", errors)
        for n in extra:
            fail(f"texts_{code}.h: extra .{n} (not in struct ui_texts)", errors)
        # Order check: translators copy the file, so drift shows up here.
        ordered = [n for n in seen if n in fields]
        want = [f for f in fields if f in values]
        if ordered != want:
            fail(f"texts_{code}.h: entries out of struct order", errors)
        tables[code] = values

    # Every listed language needs its table file, its UI_LANG_LIST entry
    # (which drives the enum and the texts() switch), and its table function.
    # The switch itself derives from the list, so a missing table is a
    # compile error; what is checked here is presence on both sides.
    dispatch = TEXTS_H.read_text(encoding="utf-8")
    lang_h = LANG_H.read_text(encoding="utf-8")
    for code in codes:
        if f'texts_{code}.h' not in dispatch:
            fail(f"src/ui/texts.h: missing #include for texts_{code}.h", errors)
        if not re.search(rf"X\(\s*{code}\s*,", lang_h):
            fail(f"src/ui/lang.h: UI_LANG_LIST has no X({code}, ...)", errors)
    for path in sorted((ROOT / "src/ui").glob("texts_*.h")):
        code = path.stem[len("texts_"):]
        if code not in codes:
            fail(f"src/ui/texts_{code}.h: no UI_LANG_LIST entry, table unreachable", errors)
            continue
        if f"{code}_texts(" not in path.read_text(encoding="utf-8"):
            fail(f"src/ui/texts_{code}.h: missing {code}_texts() table", errors)

    # Printf formats must match the ja reference, in order.
    if "ja" in tables:
        ref = tables["ja"]
        for code, values in tables.items():
            if code == "ja":
                continue
            for f in fields:
                if f not in ref or f not in values:
                    continue
                want = printf_specs(ref[f])
                got = printf_specs(values[f])
                if want != got:
                    fail(
                        f"texts_{code}.h: .{f} formats {got} != ja {want}",
                        errors,
                    )

    # UI_TEXT(id, "default") sites: the id must exist, and the default must
    # equal the en entry (semantic compare, so line splits may differ).
    if "en" in tables:
        en = tables["en"]
        for name, uses in sorted(ui_text_sites().items()):
            if not re.fullmatch(r"\w+", name):
                fail(f"UI_TEXT has a bad id: {name!r}", errors)
                continue
            if name not in fields:
                try:
                    first_path, first_line, default = uses[0]
                except IndexError:
                    continue
                fail(f"UI_TEXT({name}) at {first_path}:{first_line} "
                     f"has no struct member; add one, then:", errors)
                print(f'      const char *{name};')
                print(f'      .{name} = "{default}",  // TODO: translate')
                print(f'      .{name} = "{default}",')
                continue
            for path, lineno, default in uses:
                if unescape(default) != unescape(en[name]):
                    fail(f"UI_TEXT({name}) at {path}:{lineno} default "
                         f"differs from texts_en.h", errors)

    # fx_category_label() in xg_ui.cpp matches xg categories by their
    # Japanese name: every fxcat_* field's ja text must name a real
    # xg category (a typo would silently leave that menu Japanese).
    if "ja" in tables:
        cats = re.findall(r'\{\s*"([^"]+)"\s*,\s*\{[0-9a-fx,\s]+\}\s*\},?',
                          (ROOT / "src/xg/fx_types.h").read_text(encoding="utf-8"))
        cats = [c for c in cats if re.search(r"[぀-ヿ一-鿿]", c)]
        ja = tables["ja"]
        for f in fields:
            if f.startswith("fxcat_") and f != "fxcat_other":
                if ja.get(f) not in cats:
                    fail(f"texts_ja.h: .{f} names no xg category", errors)
        for c in cats:
            if c not in [ja.get(f) for f in fields if f.startswith("fxcat_")]:
                fail(f"xg category {c!r} has no fxcat_* field", errors)

    if errors:
        print(f"\n{len(errors)} problem(s)")
        return 1
    print("\nOK: all languages define all fields, formats match")
    return 0


if __name__ == "__main__":
    sys.exit(main())
