#!/usr/bin/env python3
"""Cross-check the probe's hand-written field lists against the *installed* LMU SDK headers.

probe.cpp lists every field by hand (offsetof/sizeof per field). This script independently
parses the struct/enum bodies out of the installed headers (read in place, never copied)
and verifies that, for every probed struct, the JSON contains exactly the header's data
members in declaration order, and that every probed enum has exactly the header's
enumerators. Standard library only.

usage: crosscheck_fields.py <lmu_abi_layout.json> [<SDK include dir>]
exit:  0 = match, 1 = mismatch, 2 = usage / parse error
"""
import json
import re
import sys
from pathlib import Path

DEFAULT_SDK = Path(
    r"C:\Program Files (x86)\Steam\steamapps\common\Le Mans Ultimate\Support\SharedMemoryInterface"
)
HEADERS = ["InternalsPlugin.hpp", "SharedMemoryInterface.hpp"]
# Structs whose definition is nested inside a class (LockData in SharedMemoryLock).
NESTED = {"SharedMemoryLock::LockData": "LockData"}


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"//[^\n]*", " ", text)


def matching_brace(text: str, open_idx: int) -> int:
    depth = 0
    for i in range(open_idx, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return i
    raise ValueError("unbalanced braces")


def find_body(text: str, keyword: str, name: str) -> str:
    # e.g. "struct TelemInfoV01 {", "struct GraphicsInfoV02 : public ... {", "enum class X : uint8_t {"
    pat = re.compile(r"\b" + keyword + r"\s+" + re.escape(name) + r"\b[^;{]*\{")
    m = pat.search(text)
    if not m:
        raise ValueError(f"{keyword} {name} not found")
    open_idx = m.end() - 1
    return text[open_idx + 1 : matching_brace(text, open_idx)]


def declarator_names(decl: str):
    decl = decl.strip()
    if not decl or "(" in decl or decl.startswith(("public", "private", "protected", "friend",
                                                    "static", "typedef", "using", "enum")):
        return []
    decl = re.sub(r"^(public|private|protected)\s*:\s*", "", decl)
    # split top-level commas (array bounds never contain commas here)
    parts = [p.strip() for p in decl.split(",")]
    names = []
    first = parts[0]
    first = re.sub(r"\[[^\]]*\]", "", first).strip()
    m = re.search(r"([A-Za-z_]\w*)\s*$", first)
    if not m:
        return []
    names.append(m.group(1))
    for p in parts[1:]:
        p = re.sub(r"\[[^\]]*\]", "", p).replace("*", "").strip()
        if re.fullmatch(r"[A-Za-z_]\w*", p):
            names.append(p)
    return names


def data_members(body: str):
    """Data members in declaration order; anonymous struct/union members are flattened."""
    members = []
    i = 0
    stmt_start = 0
    while i < len(body):
        c = body[i]
        if c == "{":
            end = matching_brace(body, i)
            head = body[stmt_start:i].strip()
            if re.fullmatch(r"(union|struct)", head):
                members.extend(data_members(body[i + 1 : end]))
                j = end + 1
                while j < len(body) and body[j] in " \t\r\n":
                    j += 1
                if j < len(body) and body[j] == ";":
                    j += 1
                i = stmt_start = j
                continue
            # function body (or initializer) -> drop the whole declaration
            j = end + 1
            while j < len(body) and body[j] in " \t\r\n":
                j += 1
            if j < len(body) and body[j] == ";":
                j += 1
            i = stmt_start = j
            continue
        if c == ";":
            members.extend(declarator_names(body[stmt_start:i]))
            stmt_start = i + 1
        i += 1
    return members


def enumerators(body: str):
    out = []
    for part in body.split(","):
        part = part.strip()
        if part:
            out.append(re.match(r"[A-Za-z_]\w*", part).group(0))
    return out


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    layout = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    sdk = Path(sys.argv[2]) if len(sys.argv) > 2 else DEFAULT_SDK
    text = "\n".join(strip_comments((sdk / h).read_text(encoding="utf-8", errors="replace")) for h in HEADERS)

    failures = 0
    for sname, srec in layout["structs"].items():
        hname = NESTED.get(sname, sname)
        header_fields = data_members(find_body(text, "struct", hname))
        json_fields = [f["name"] for f in srec["fields"]]
        if header_fields == json_fields:
            print(f"OK    struct {sname}: {len(json_fields)} fields match header declaration order")
        else:
            failures += 1
            missing = [f for f in header_fields if f not in json_fields]
            extra = [f for f in json_fields if f not in header_fields]
            print(f"FAIL  struct {sname}: header={len(header_fields)} json={len(json_fields)} "
                  f"missing={missing} extra={extra} order_ok={not missing and not extra}")

    for ename, erec in layout["enums"].items():
        keyword = "enum class" if erec.get("scoped") else "enum"
        header_vals = enumerators(find_body(text, keyword, ename))
        json_vals = list(erec["values"].keys())
        if header_vals == json_vals:
            print(f"OK    enum {ename}: {len(json_vals)} enumerators match")
        else:
            failures += 1
            print(f"FAIL  enum {ename}: header={header_vals} json={json_vals}")

    print("RESULT:", "PASS" if failures == 0 else f"FAIL ({failures})")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
