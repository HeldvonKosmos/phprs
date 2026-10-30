#!/usr/bin/env python3
"""R1: php-src's .phpt tests as probes for the dialect (D12, Phase 1).

Reads the tests from refs/vm/php-src at run time (nothing is copied into the
repository), takes the --FILE-- section, runs it with PHP 8.4 twice (the
reference; differing runs are non-deterministic and excluded) and with grd
(GRD_JIT=0), each in its own temporary directory with a time and memory
limit, and classifies the result:

  identical     same output as PHP
  dialect       rejected or changed by a dialect rule: a GRD code in grd's
                output, or an exception where PHP printed a warning/notice
                (errors are exceptions, DIALECT §5)
  gap           anything else, with a reason (missing function or class,
                different output, crash, timeout)
  rejected      PHP rejects the script at compile time and grd does too
  excluded      not runnable as a plain script (SAPI sections, INI, ARGS,
                ENV, STDIN, external files), PHP itself fails or times out,
                or PHP's two runs differ

Results go to target/refs/phpt/ (results.json, one line per test), the
summary to stdout as Markdown (for .agents/PHPT_REPORT.md).

usage: tools/phpt/run.py [--grd BIN] [--jobs 8] [--cores 0-7] [--only SUBSTR] [--limit N]
"""
import argparse, collections, json, os, re, resource, subprocess, sys, tempfile
from concurrent.futures import ProcessPoolExecutor

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
WS = os.path.dirname(REPO)
SRC = os.path.join(WS, "refs/vm/php-src")
OUT = os.path.join(REPO, "target/refs/phpt")
UNSUPPORTED = {"INI", "ARGS", "ENV", "STDIN", "POST", "POST_RAW", "GET", "COOKIE", "CGI", "FILE_EXTERNAL",
               "REDIRECTTEST", "PHPDBG", "CAPTURE_STDIO", "GZIP_POST", "DEFLATE_POST", "PUT", "FILEEOF", "HEADERS"}
PHP = ["php", "-d", "opcache.enable_cli=0", "-d", "display_errors=1", "-d", "error_reporting=-1", "-d", "html_errors=0"]
TIMEOUT = 10


def sections(path):
    out, cur = {}, None
    for line in open(path, "rb").read().split(b"\n"):
        m = re.match(rb"^--([A-Z_]+)--\s*$", line)
        if m:
            cur = m.group(1).decode()
            out[cur] = []
        elif cur:
            out[cur].append(line)
    return {k: b"\n".join(v) for k, v in out.items()}


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (2 << 30, 2 << 30))
    resource.setrlimit(resource.RLIMIT_CPU, (TIMEOUT, TIMEOUT + 1))


def run(cmd, cwd, env=None):
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, timeout=TIMEOUT, preexec_fn=limits, env=env)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return None, b""


def norm(out, path):
    s = out.replace(path.encode(), b"%s").replace(os.path.basename(path).encode(), b"%s")
    return re.sub(rb"[ \t]+\n", b"\n", s).strip()


def classify(rel, a):
    s = sections(os.path.join(SRC, rel))
    if "FILE" not in s:
        return {"test": rel, "kind": "excluded", "reason": "no --FILE--"}
    bad = sorted(set(s) & UNSUPPORTED)
    if bad:
        return {"test": rel, "kind": "excluded", "reason": "section " + bad[0]}
    with tempfile.TemporaryDirectory(prefix="phpt-") as d:
        f = os.path.join(d, "t.php")
        open(f, "wb").write(s["FILE"])
        r1, o1 = run(["taskset", "-c", a.cores] + PHP + [f], d)
        if r1 is None:
            return {"test": rel, "kind": "excluded", "reason": "PHP timeout"}
        r2, o2 = run(["taskset", "-c", a.cores] + PHP + [f], d)
        if norm(o1, f) != norm(o2, f):
            return {"test": rel, "kind": "excluded", "reason": "PHP not deterministic"}
        env = dict(os.environ, GRD_JIT="0")
        rg, og = run(["taskset", "-c", a.cores, a.grd, "run", f], d, env)
        # PHP rejects the script (parse or compile-time fatal error): grd must reject it too
        if re.search(rb"^(PHP )?(Parse error|Fatal error)", o1, re.M) and b"Uncaught" not in o1:
            if rg is not None and rg != 0 and re.search(rb"error|Fatal|GRD\d{3}", og):
                return {"test": rel, "kind": "rejected", "reason": "both reject"}
            return {"test": rel, "kind": "gap", "reason": "PHP rejects, grd accepts"}
    php, grd = norm(o1, f), norm(og, f) if rg is not None else b""
    if rg is None:
        return {"test": rel, "kind": "gap", "reason": "grd timeout"}
    if php == grd:
        return {"test": rel, "kind": "identical"}
    text = grd.decode(errors="replace")
    code = re.search(r"GRD\d{3}", text)
    if code:
        return {"test": rel, "kind": "dialect", "reason": code.group(0)}
    m = re.search(r"Call to undefined function ([\w\\]+)\(\)", text)
    if m:
        return {"test": rel, "kind": "gap", "reason": "missing function " + m.group(1)}
    m = re.search(r"Class \"?([\w\\]+)\"? not found|Undefined class ([\w\\]+)", text)
    if m:
        return {"test": rel, "kind": "gap", "reason": "missing class " + (m.group(1) or m.group(2))}
    if rg not in (0, 255) or b"panicked" in grd:
        return {"test": rel, "kind": "gap", "reason": f"grd crash (exit {rg})"}
    php_t = php.decode(errors="replace")
    if re.search(r"^(Warning|Notice|Deprecated):", php_t, re.M) and re.search(r"Uncaught|Error|Exception", text):
        return {"test": rel, "kind": "dialect", "reason": "exception instead of warning (§5)"}
    first = next((i for i, (x, y) in enumerate(zip(php_t.split("\n"), text.split("\n"))) if x != y), None)
    return {"test": rel, "kind": "gap", "reason": "different output", "line": first,
            "php": php_t.split("\n")[first][:120] if first is not None and first < len(php_t.split("\n")) else "",
            "grd": text.split("\n")[first][:120] if first is not None and first < len(text.split("\n")) else ""}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--grd", default=os.path.join(WS, "target/profiling/grd"))
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--cores", default="0-7")
    ap.add_argument("--only", default="")
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()
    a.grd = os.path.abspath(a.grd)
    tests = sorted(os.path.relpath(os.path.join(dp, f), SRC) for dp, _, fs in os.walk(SRC) for f in fs if f.endswith(".phpt"))
    tests = [t for t in tests if a.only in t]
    if a.limit:
        tests = tests[: a.limit]
    os.makedirs(OUT, exist_ok=True)
    with ProcessPoolExecutor(a.jobs) as ex:
        results = list(ex.map(classify, tests, [a] * len(tests), chunksize=8))
    with open(os.path.join(OUT, "results.json"), "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    report(results)


def directory(t):
    parts = t.split("/")
    return "/".join(parts[:3]) if parts[0] == "ext" else "/".join(parts[:2]) if len(parts) > 2 else parts[0]


def report(results):
    kinds = ["identical", "rejected", "dialect", "gap", "excluded"]
    total = collections.Counter(r["kind"] for r in results)
    runnable = total["identical"] + total["rejected"] + total["dialect"] + total["gap"]
    sh = lambda c: subprocess.run(c, capture_output=True, text=True).stdout.strip()
    print(f"Stand: phprs {sh(['git', '-C', REPO, 'rev-parse', '--short', 'HEAD'])}, php-src php-8.4.21, PHP {sh(['php', '-r', 'echo PHP_VERSION;'])}; "
          f"{len(results)} tests, {runnable} of them runnable.\n")
    print("| | Tests | Share of runnable |\n|---|---:|---:|")
    for k, name in zip(kinds, ["identical", "both reject (compile error)", "dialect-related", "gap", "excluded"]):
        share = f"{100 * total[k] / runnable:.1f} %" if k != "excluded" and runnable else "–"
        print(f"| {name} | {total[k]} | {share} |")
    print("\n### Per directory\n\n| Directory | Runnable | Identical | Both reject | Dialect-related | Gap | Excluded |\n|---|---:|---:|---:|---:|---:|---:|")
    per = collections.defaultdict(collections.Counter)
    for r in results:
        per[directory(r["test"])][r["kind"]] += 1
    for d, c in sorted(per.items(), key=lambda x: -sum(x[1].values())):
        run_ = c["identical"] + c["rejected"] + c["dialect"] + c["gap"]
        print(f"| {d} | {run_} | {c['identical']} | {c['rejected']} | {c['dialect']} | {c['gap']} | {c['excluded']} |")
    for k, title in [("dialect", "Dialect-related by reason"), ("gap", "Gaps by reason"), ("excluded", "Excluded by reason")]:
        reasons = collections.Counter(re.sub(r"^(missing function|missing class) .*", r"\1", r["reason"]) for r in results if r["kind"] == k)
        print(f"\n### {title}\n\n| Grund | Tests |\n|---|---:|")
        for reason, n in reasons.most_common(20):
            print(f"| {reason} | {n} |")
    for label, prefix in [("Fehlende Funktionen", "missing function "), ("Fehlende Klassen", "missing class ")]:
        c = collections.Counter(r["reason"][len(prefix):] for r in results if r.get("reason", "").startswith(prefix))
        print(f"\n### {label} (most frequent 40)\n\n" + ", ".join(f"`{n}` ({k})" for n, k in c.most_common(40)))


if __name__ == "__main__":
    main()
