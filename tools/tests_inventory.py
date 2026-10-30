#!/usr/bin/env python3
"""Inventory of the tests (TEST1 step 1, `.agents/Tools.md`).

Lists every Rust test function (unit tests under src/, integration tests
under tests/) with its id, whether it is ignored, and the generated test
groups (probes, case files) with their size. With --check FILE it compares
the inventory with the descriptions in FILE: a test is described when its id
appears in backticks there (`grd-vm:tests/suite/types.rs::name`); the script
reports tests without a description and described ids without a test, and
exits 1 if there are any.

usage: tools/tests_inventory.py [--check ../.agents/Tools.md] [--markdown]
"""
import argparse, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, ".."))

TEST_ATTR = re.compile(r"#\[(?:tokio::)?test(?:\([^)]*\))?\]")
FN = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)")


def crate_name(crate_dir):
    for line in open(os.path.join(crate_dir, "Cargo.toml")):
        m = re.match(r'\s*name\s*=\s*"([^"]+)"', line)
        if m:
            return m.group(1)
    return os.path.basename(crate_dir)


def rust_tests():
    """(id, crate, file, fn, kind, ignored, reason)"""
    out = []
    crates = os.path.join(REPO, "crates")
    for c in sorted(os.listdir(crates)):
        cdir = os.path.join(crates, c)
        if not os.path.isfile(os.path.join(cdir, "Cargo.toml")):
            continue
        name = crate_name(cdir)
        for root, _, files in os.walk(cdir):
            if "/target" in root or "/fixtures" in root:
                continue
            for f in sorted(files):
                if not f.endswith(".rs"):
                    continue
                path = os.path.join(root, f)
                rel = os.path.relpath(path, cdir)
                kind = "integration" if rel.startswith("tests/") else "unit"
                lines = open(path, encoding="utf-8", errors="replace").read().split("\n")
                i = 0
                while i < len(lines):
                    if TEST_ATTR.search(lines[i]):
                        ignored, reason = False, ""
                        j = i + 1
                        # attributes between #[test] and fn (#[ignore = "…"], #[cfg…])
                        while j < len(lines) and not FN.match(lines[j]):
                            m = re.search(r'#\[ignore(?:\s*=\s*"([^"]*)")?\]', lines[j])
                            if m:
                                ignored, reason = True, m.group(1) or ""
                            j += 1
                        # #[ignore] may also stand before #[test]
                        k = i - 1
                        while k >= 0 and lines[k].strip().startswith("#["):
                            m = re.search(r'#\[ignore(?:\s*=\s*"([^"]*)")?\]', lines[k])
                            if m:
                                ignored, reason = True, m.group(1) or ""
                            k -= 1
                        if j < len(lines):
                            fn = FN.match(lines[j]).group(1)
                            tid = f"{name}:{rel}::{fn}"
                            out.append((tid, name, rel, fn, kind, ignored, reason))
                        i = j
                    i += 1
    return out


def count_lines(path, skip_blank=True):
    try:
        return sum(1 for l in open(path, encoding="utf-8", errors="replace") if l.strip() or not skip_blank)
    except OSError:
        return 0


def groups():
    """Generated or data-driven groups: (id, what, size)."""
    g = []
    probes = os.path.join(REPO, "crates/vm/tests/probes")
    if os.path.isdir(probes):
        grd = [f for f in os.listdir(probes) if f.endswith(".grd")]
        outs = [f for f in os.listdir(probes) if f.endswith(".out")]
        g.append(("group:probes", "VM probes (crates/vm/tests/probes/*.grd with .out reference)", f"{len(grd)} .grd, {len(outs)} .out"))
    sup = os.path.join(REPO, "crates/vm/tests/support/cases.txt")
    if os.path.isfile(sup):
        g.append(("group:support-cases", "Support spec (crates/vm/tests/support/cases.txt / cases.out)", f"{count_lines(sup)} cases"))
    vdir = os.path.join(REPO, "crates/framework/tests/validation_spec")
    if os.path.isdir(vdir):
        files = sorted(os.listdir(vdir))
        g.append(("group:validation-cases", "Validation spec (crates/framework/tests/validation_spec/)", ", ".join(files)))
    tdir = os.path.join(REPO, "crates/vm/tests/types")
    if os.path.isdir(tdir):
        g.append(("group:type-matrix", "Type matrix data (crates/vm/tests/types/)", ", ".join(sorted(os.listdir(tdir)))))
    fx = os.path.join(REPO, "crates/vm/tests/fixtures")
    if os.path.isdir(fx):
        n = sum(len(fs) for _, _, fs in os.walk(fx))
        g.append(("group:vm-fixtures", "VM fixtures (crates/vm/tests/fixtures/)", f"{n} files"))
    api = os.path.join(REPO, "examples/test-app/tests/api.sh")
    if os.path.isfile(api):
        steps = sum(1 for l in open(api) if re.search(r'^\s*(check|pass|fail|skip)\s+"', l))
        g.append(("group:api-sh", "Acceptance test examples/test-app/tests/api.sh", f"{steps} step calls"))
    return g


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", help="Tools.md to compare with")
    ap.add_argument("--markdown", action="store_true", help="print the inventory as a Markdown table")
    a = ap.parse_args()
    tests = rust_tests()
    gs = groups()
    ids = [t[0] for t in tests]
    dups = sorted({i for i in ids if ids.count(i) > 1})
    if a.markdown:
        print("| Id | Kind | Skipped |")
        print("|---|---|---|")
        for t in tests:
            print(f"| `{t[0]}` | {t[4]} | {'ja: ' + t[6] if t[5] else ''} |")
        print()
        print("| Gruppe | Inhalt | Umfang |")
        print("|---|---|---|")
        for gid, what, size in gs:
            print(f"| `{gid}` | {what} | {size} |")
    else:
        by = {}
        for t in tests:
            by.setdefault((t[1], t[4]), 0)
            by[(t[1], t[4])] += 1
        for (c, k), n in sorted(by.items()):
            print(f"{c:22} {k:12} {n:4}")
        print(f"{'total':35} {len(tests):4} test functions, {sum(t[5] for t in tests)} ignored")
        for gid, what, size in gs:
            print(f"{gid:24} {size}")
    if dups:
        print("duplicate ids: " + ", ".join(dups), file=sys.stderr)
    if a.check:
        text = open(a.check, encoding="utf-8").read()
        described = set(re.findall(r"`((?:[a-z0-9-]+:[A-Za-z0-9_./-]+\.rs::[A-Za-z0-9_]+)|(?:group:[a-z0-9-]+))`", text))
        known = set(ids) | {g[0] for g in gs}
        missing = sorted(known - described)
        stale = sorted(described - known)
        for m in missing:
            print(f"without description: {m}")
        for s in stale:
            print(f"description without test: {s}")
        print(f"check: {len(known) - len(missing)} of {len(known)} described, {len(stale)} stale")
        sys.exit(1 if missing or stale else 0)


if __name__ == "__main__":
    main()
