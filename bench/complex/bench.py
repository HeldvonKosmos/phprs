#!/usr/bin/env python3
"""Script benchmarks c1-c3: PHP 8.4 against the dialect (method D11, BENCHMARKS.md).

Comparisons, always reported separately:
  A  interpreter vs interpreter: php -d opcache.enable_cli=0 vs grd with GRD_JIT=0
  B  php with opcache, JIT off vs grd with GRD_JIT=0
  C  JIT vs JIT: php with opcache tracing JIT vs grd with its JIT
Both sides compile the source on every run (grd has no bytecode cache; PHP CLI
keeps opcache only in process memory). Phases come from the scripts' own
hrtime output; wall time, process start-up (empty script) and, for A, the
compile time of the script (batch of 100 files, minus an empty batch) are
measured here. Runs are interleaved, pinned with taskset, median and relative
standard deviation over --runs runs. Checksums must match.

With --grd-pgo BIN (PGO1) the PGO binary is measured in the same interleaved
runs and every table gets its own columns for it.

usage: bench.py [--grd BIN] [--grd-pgo BIN] [--runs 7] [--cores 8,10,12,14] [--compare A,B,C] [--bench c1_request,...]
"""
import argparse, datetime, os, statistics, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
PGO_BIN = None
PHP_A = ["php", "-d", "opcache.enable_cli=0"]
PHP_B = ["php", "-d", "opcache.enable_cli=1", "-d", "opcache.jit=disable"]
PHP_C = ["php", "-d", "opcache.enable_cli=1", "-d", "opcache.jit=tracing", "-d", "opcache.jit_buffer_size=64M"]
COMPARE = {
    "A": ("PHP without opcache, without JIT", "php-a", "grd without JIT (GRD_JIT=0)", "grd-int"),
    "B": ("PHP with opcache, without JIT", "php-b", "grd without JIT (GRD_JIT=0)", "grd-int"),
    "C": ("PHP with opcache JIT (tracing)", "php-c", "grd with JIT", "grd-jit"),
}


def sh(cmd):
    return subprocess.run(cmd, capture_output=True, text=True).stdout.strip()


def variant_cmd(v, grd, script):
    php = {"php-a": PHP_A, "php-b": PHP_B, "php-c": PHP_C}
    if v in php:
        return php[v] + [script + ".php"], None
    if v.endswith("-pgo"):
        grd, v = PGO_BIN, v[:-4]
    env = dict(os.environ)
    if v == "grd-int":
        env["GRD_JIT"] = "0"
    else:
        env.pop("GRD_JIT", None)
    return [grd, "run", script + ".grd"], env


def timed(cmd, env, cores):
    t0 = time.perf_counter()
    p = subprocess.run(["taskset", "-c", cores] + cmd, capture_output=True, text=True, env=env, cwd=HERE)
    wall = (time.perf_counter() - t0) * 1000
    if p.returncode != 0:
        sys.exit(f"failed: {' '.join(cmd)}\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}")
    return wall, p.stdout


def parse(out):
    d = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 2:
            d[parts[0]] = parts[1]
    return d


def stat(xs):
    m = statistics.median(xs)
    sd = statistics.stdev(xs) / statistics.mean(xs) * 100 if len(xs) > 1 and statistics.mean(xs) else 0.0
    return m, sd


def compile_ms(v, grd, script, runs, cores, tmp):
    """Compile time of one file: (batch of 100 copies - batch of 100 empty files) / 100."""
    n = 100
    if v == "php-a":
        src, empty, tool, env = script + ".php", os.path.join(tmp, "empty.php"), PHP_A + ["-l"], None
    else:
        src, empty, tool, env = script + ".grd", os.path.join(tmp, "empty.grd"), [grd, "compile"], None
    src = os.path.join(HERE, src)
    full, base = [], []
    for _ in range(runs):
        full.append(timed(tool + [src] * n, env, cores)[0])
        base.append(timed(tool + [empty] * n, env, cores)[0])
    return stat([(f - b) / n for f, b in zip(full, base)])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--grd", default=os.path.join(HERE, "../../../target/profiling/grd"))
    ap.add_argument("--grd-pgo", default=None)
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--cores", default="8,10,12,14")
    ap.add_argument("--compare", default="A,B,C")
    ap.add_argument("--bench", default="c1_request,c2_runtime,c3_compute")
    a = ap.parse_args()
    global PGO_BIN
    grd = os.path.abspath(a.grd)
    PGO_BIN = os.path.abspath(a.grd_pgo) if a.grd_pgo else None
    comps = a.compare.split(",")
    variants = []
    for c in comps:
        for v in (COMPARE[c][1], COMPARE[c][3]) + ((COMPARE[c][3] + "-pgo",) if PGO_BIN else ()):
            if v not in variants:
                variants.append(v)

    modules = sh(["php", "-m"])
    if "xdebug" in modules.lower():
        sys.exit("Xdebug is loaded; the method forbids it")
    repo = os.path.join(HERE, "../..")
    commit = sh(["git", "-C", repo, "rev-parse", "--short", "HEAD"])
    dirty = " (+ uncommitted changes)" if sh(["git", "-C", repo, "status", "--porcelain", "--", "crates"]) else ""
    gov = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_governor"]) or "?"
    cap = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_max_freq"])
    gov += f", max {int(cap) / 1e6:.2f} GHz" if cap.isdigit() else ""
    print(f"Commit {commit}{dirty} · {datetime.datetime.now():%Y-%m-%d %H:%M} · {sh(['php', '-r', 'echo PHP_VERSION;'])} (NTS, ohne Xdebug)"
          f" · grd {os.path.relpath(grd, repo)}" + (f" · PGO {os.path.relpath(PGO_BIN, repo)}" if PGO_BIN else "") + f" · cores {a.cores} · governor {gov} · median of {a.runs} runs, relative σ\n")

    with tempfile.TemporaryDirectory() as tmp:
        open(os.path.join(tmp, "empty.php"), "w").write("<?php\n")
        open(os.path.join(tmp, "empty.grd"), "w").write("<?grd\n")
        startup = {}
        for v in variants:
            cmd, env = variant_cmd(v, grd, os.path.join(tmp, "empty"))
            startup[v] = stat([timed(cmd, env, a.cores)[0] for _ in range(a.runs)])
        for b in a.bench.split(","):
            res = {v: [] for v in variants}
            for _ in range(a.runs):
                for v in variants:
                    cmd, env = variant_cmd(v, grd, b)
                    wall, out = timed(cmd, env, a.cores)
                    d = parse(out)
                    d["_wall"] = wall
                    res[v].append(d)
            sums = {v: {r.get("checksum") for r in res[v]} for v in variants}
            ok = len(set().union(*sums.values())) == 1 and None not in set().union(*sums.values())
            phases = [k for k in res[variants[0]][0] if k not in ("checksum", "_wall", "total")]
            comp = {}
            if "A" in comps:
                comp = {v: compile_ms(v, grd, b, a.runs, a.cores, tmp) for v in ("php-a", "grd-int")}
            for c in comps:
                ln, lv, rn, rv = COMPARE[c]
                print(f"### {b} · Vergleich {c}: {ln} gegen {rn}")
                print(f"Checksums: {'equal' if ok else 'DIFFERENT ' + str(sums)} ({'/'.join(sorted(sums[lv]))})\n")
                pv = rv + "-pgo" if PGO_BIN else None
                print("| Phase | PHP ms | σ | grd ms | σ | grd/PHP |" + (" PGO ms | σ | PGO/PHP |" if pv else ""))
                print("|---|---:|---:|---:|---:|---:|" + ("---:|---:|---:|" if pv else ""))

                def row(name, l, r, g=None):
                    f = lambda r: f"{r[0] / l[0]:.1f}×" if l[0] > 0 else "–"
                    n = lambda x: f"{x:.2f}" if abs(x) < 10 else f"{x:.1f}"
                    extra = f" {n(g[0])} | {g[1]:.0f} % | {f(g)} |" if g else ""
                    print(f"| {name} | {n(l[0])} | {l[1]:.0f} % | {n(r[0])} | {r[1]:.0f} % | {f(r)} |" + extra)
                pg = lambda fn: fn(pv) if pv else None
                for p in phases:
                    col = lambda v: stat([float(x[p]) for x in res[v]])
                    row(p, col(lv), col(rv), pg(col))
                tot = lambda v: stat([sum(float(x[p]) for p in phases) for x in res[v]])
                row("**Summe der Phasen**", tot(lv), tot(rv), pg(tot))
                if c == "A":
                    row("Compiling the script", comp["php-a"], comp["grd-int"])
                row("Prozessstart (leeres Skript)", startup[lv], startup[rv], pg(lambda v: startup[v]))
                rest = lambda v, x: x["_wall"] - sum(float(x[p]) for p in phases) - startup[v][0] - (comp[v][0] if c == "A" and v in comp else 0)
                restc = lambda v: stat([rest(v, x) for x in res[v]])
                row("remaining time (data generation, output, teardown)", restc(lv), restc(rv), pg(restc))
                wall = lambda v: stat([x["_wall"] for x in res[v]])
                row("**Wanduhr gesamt**", wall(lv), wall(rv), pg(wall))
                print()
            if not ok:
                sys.exit(1)


if __name__ == "__main__":
    main()
