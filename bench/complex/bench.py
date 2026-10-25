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

usage: bench.py [--grd BIN] [--runs 7] [--cores 8-11] [--compare A,B,C] [--bench c1_request,...]
"""
import argparse, datetime, os, statistics, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
PHP_A = ["php", "-d", "opcache.enable_cli=0"]
PHP_B = ["php", "-d", "opcache.enable_cli=1", "-d", "opcache.jit=disable"]
PHP_C = ["php", "-d", "opcache.enable_cli=1", "-d", "opcache.jit=tracing", "-d", "opcache.jit_buffer_size=64M"]
COMPARE = {
    "A": ("PHP ohne Opcache, ohne JIT", "php-a", "grd ohne JIT (GRD_JIT=0)", "grd-int"),
    "B": ("PHP mit Opcache, ohne JIT", "php-b", "grd ohne JIT (GRD_JIT=0)", "grd-int"),
    "C": ("PHP mit Opcache-JIT (tracing)", "php-c", "grd mit JIT", "grd-jit"),
}


def sh(cmd):
    return subprocess.run(cmd, capture_output=True, text=True).stdout.strip()


def variant_cmd(v, grd, script):
    php = {"php-a": PHP_A, "php-b": PHP_B, "php-c": PHP_C}
    if v in php:
        return php[v] + [script + ".php"], None
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
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--cores", default="8-11")
    ap.add_argument("--compare", default="A,B,C")
    ap.add_argument("--bench", default="c1_request,c2_runtime,c3_compute")
    a = ap.parse_args()
    grd = os.path.abspath(a.grd)
    comps = a.compare.split(",")
    variants = []
    for c in comps:
        for v in (COMPARE[c][1], COMPARE[c][3]):
            if v not in variants:
                variants.append(v)

    modules = sh(["php", "-m"])
    if "xdebug" in modules.lower():
        sys.exit("Xdebug is loaded; the method forbids it")
    repo = os.path.join(HERE, "../..")
    commit = sh(["git", "-C", repo, "rev-parse", "--short", "HEAD"])
    dirty = " (+ uncommitted changes)" if sh(["git", "-C", repo, "status", "--porcelain", "--", "crates"]) else ""
    gov = sh(["cat", "/sys/devices/system/cpu/cpu8/cpufreq/scaling_governor"]) or "?"
    print(f"Commit {commit}{dirty} · {datetime.datetime.now():%Y-%m-%d %H:%M} · {sh(['php', '-r', 'echo PHP_VERSION;'])} (NTS, ohne Xdebug)"
          f" · grd {os.path.relpath(grd, repo)} · Kerne {a.cores} · Governor {gov} · Median aus {a.runs} Läufen, σ relativ\n")

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
                print(f"Prüfsummen: {'gleich' if ok else 'UNGLEICH ' + str(sums)} ({'/'.join(sorted(sums[lv]))})\n")
                print(f"| Phase | PHP ms | σ | grd ms | σ | grd/PHP |")
                print(f"|---|---:|---:|---:|---:|---:|")

                def row(name, l, r):
                    f = f"{r[0] / l[0]:.1f}×" if l[0] > 0 else "–"
                    n = lambda x: f"{x:.2f}" if abs(x) < 10 else f"{x:.1f}"
                    print(f"| {name} | {n(l[0])} | {l[1]:.0f} % | {n(r[0])} | {r[1]:.0f} % | {f} |")
                for p in phases:
                    row(p, stat([float(x[p]) for x in res[lv]]), stat([float(x[p]) for x in res[rv]]))
                tot = lambda v: stat([sum(float(x[p]) for p in phases) for x in res[v]])
                row("**Summe der Phasen**", tot(lv), tot(rv))
                if c == "A":
                    row("Übersetzung des Skripts", comp["php-a"], comp["grd-int"])
                row("Prozessstart (leeres Skript)", startup[lv], startup[rv])
                rest = lambda v, x: x["_wall"] - sum(float(x[p]) for p in phases) - startup[v][0] - (comp[v][0] if c == "A" else 0)
                row("übrige Zeit (Daten erzeugen, Ausgabe, Abbau)", stat([rest(lv, x) for x in res[lv]]), stat([rest(rv, x) for x in res[rv]]))
                row("**Wanduhr gesamt**", stat([x["_wall"] for x in res[lv]]), stat([x["_wall"] for x in res[rv]]))
                print()
            if not ok:
                sys.exit(1)


if __name__ == "__main__":
    main()
