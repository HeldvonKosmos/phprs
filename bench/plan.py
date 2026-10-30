#!/usr/bin/env python3
"""Plan of `bench/run_all.sh` (BENCH2, BENCH3): every stage, what it measures
and its estimated duration, from the same parameters and target lists the
stages use (bench/framework/targets.py; keep in sync with run_all.sh;
`stages.txt` of a run has the real times).

  bench/plan.py [--runs 3] [--with-lang] [--smoke] [--frameworks a,b] [--resume DIR] [--warmup S]
Estimates: measured time per point (warm-up + duration) is exact; setup
per target and the builds are taken from earlier runs (77d4cc5: BM2 with
11 targets 36 min; builds of 27.10.; the comparison apps' start, gate and
hot reload times of 28.10.). The warm-up is the one warmcheck chose
(<DIR>/warmup.txt with --resume), else --warmup (default 3 s, the value
before the check); the table also shows the total for longer warm-ups.
"""

import argparse, os, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "framework"))
import bm2  # noqa: E402
import targets  # noqa: E402

SETUP_S = 15  # start, schema, seed per target (earlier runs: 10-20 s)
# per framework: start of one target in s (gates, sqlite check, cold start) and hot reload stage in s
START_S = {"opengrid": 3, "laravel": 5, "adonis": 3, "goravel": 2, "rails": 4, "fastapi": 3, "spring": 6, "dotnet": 3, "phoenix": 4}
HOT_S = {"adonis": 15, "goravel": 30, "rails": 15, "fastapi": 15, "spring": 120, "dotnet": 90, "phoenix": 60}
THESIS_ENDPOINTS = 3 + 4 * 3  # n = 0 (3 bases) and 10/100/1000 per workload (compact, 28.10.)


def stages(a, warm):
    point = 2 if a.smoke else warm + 10  # warm-up + measurement of one point (bm2, load)
    fws = a.frameworks
    n_bm2 = sum(len(targets.FRAMEWORKS[f][1]) for f in fws)
    per = lambda st: sum(len(targets.FIXED.get(st, {}).get(f, [None])) for f in fws)
    rows = []
    rows.append(("gates", f"api.sh per target: {n_bm2} targets ({len(fws)} frameworks, every server variant)", sum(len(targets.FRAMEWORKS[f][1]) * (25 if f in ("opengrid", "laravel") else 20 + START_S[f]) for f in fws)))
    if not a.framework_only:
        rows.append(("lang", "language c1–c3, comparisons A/B/C, with PGO, " + ("1 run" if a.smoke else "median of 7"), 40 if a.smoke else 180))
    apps = [f for f in fws if f not in ("opengrid", "laravel")]
    rows.append(("sqlite", f"SQLite 3.53.4 + compile options: {sum(len(targets.FRAMEWORKS[f][1]) for f in apps)} app variants (abort on mismatch)", sum(len(targets.FRAMEWORKS[f][1]) * (START_S[f] + 4) for f in apps)))
    n = n_bm2 * len(bm2.ENDPOINTS) * 3
    rows.append(("bm2", f"(a) BM2: {n_bm2} targets (default variants) × {len(bm2.ENDPOINTS)} endpoints × 1/16/64 connections = {n} points", n * point + n_bm2 * SETUP_S))
    t = per("thesis")
    n = t * THESIS_ENDPOINTS
    rows.append(("thesis", f"THESIS1: {t} targets × {THESIS_ENDPOINTS} logic endpoints (n = 0/10/100/1000) × 16 connections = {n} points", n * point + t * 20))
    lt, le = per("load"), 1
    one = 3 * point + 4 * point + (6 if a.smoke else 19)
    rows.append(("load", f"(g/h) concurrency/open-loop load: {lt} targets × `show` × (3 closed + 4 levels + spike)", lt * le * one + lt * SETUP_S))
    cv = per("load")
    rows.append(("curve", f"warm-up curve: {cv} targets × hydrate:100, fresh server, 30 s of load without warm-up, requests per second", cv * (SETUP_S + (5 if a.smoke else 30) + 2)))
    m = per("memory")
    rows.append(("memory", f"memory PSS: {m} targets (idle, load, after 10 000 requests; Spring with a fixed heap)", m * (SETUP_S + (8 if a.smoke else 25))))
    runs = 1 if a.smoke else 5
    cold = 0
    for f in fws:
        for _ in targets.FIXED["coldstart"].get(f, [None]):
            cases = 4 if f == "laravel" else 2  # worker counts × (Laravel: with/without caches)
            cold += SETUP_S + cases * runs * (START_S[f] + 2) + (10 if f in ("opengrid", "laravel") else 0)
    rows.append(("coldstart", f"cold start: {per('coldstart')} targets, 4/16 workers, Laravel with/without caches, first request; hot reload OpenGrid/Laravel", cold))
    if apps:
        rows.append(("hotreload", f"hot reload in development mode: {len(apps)} frameworks, 3 changes each (marked: development mode)", sum(HOT_S[f] for f in apps)))
    rows.append(("report", "tables from the raw data (report.py)", 5))
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--with-lang", action="store_true", help="the interpreter benchmark too (off by default since 28.10.)")
    ap.add_argument("--smoke", action="store_true")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--frameworks", default=",".join(targets.FRAMEWORKS))
    ap.add_argument("--resume", help="result directory of an aborted series (its warmup.txt)")
    ap.add_argument("--warmup", type=int, default=3)
    a = ap.parse_args()
    a.framework_only = not a.with_lang
    a.frameworks = [f for f in a.frameworks.replace(" ", ",").split(",") if f]
    warm, source = a.warmup, "assumed (before the check)"
    if a.resume and os.path.exists(os.path.join(a.resume, "warmup.txt")):
        warm, source = int(open(os.path.join(a.resume, "warmup.txt")).read().strip()), "from warmcheck"
    build = 45 * 60 if a.framework_only else 60 * 60  # perf build 17 min + PGO (instrumented build, training, perf build), measured 28.10.: 26-45 min
    n_targets = sum(len(targets.FRAMEWORKS[f][1]) for f in a.frameworks)
    warmcheck = n_targets * (SETUP_S + 3 + 30) + (0 if a.smoke else 5 * (SETUP_S + 10 + 30))  # one endpoint (list); ~5 targets need a longer step

    def total(w):
        rows = stages(a, w)
        return build + (0 if a.smoke else warmcheck) + sum(r[2] for r in rows) * a.runs

    rows = stages(a, warm)
    print(f"Plan of bench/run_all.sh --runs {a.runs}{'' if a.framework_only else ' --with-lang'}{' --smoke' if a.smoke else ''} "
          f"(estimate; build and warm-up check once, every stage per run; warm-up {warm} s per point, {source})\n")
    print("| Stage | Content | Estimated per run |")
    print("|---|---|---:|")
    what = "perf opengrid (fat LTO, target-cpu=native), PGO opengrid" if a.framework_only else "profiling grd, perf opengrid, PGO grd and opengrid"
    print(f"| build | binaries from HEAD: {what} (bench/pgo.sh); once | {build / 60:.0f} min |")
    if not a.smoke:
        print(f"| warmcheck | warm-up check: {n_targets} targets × `list`, 3 × 10 s ≤ 3 %, longer step where needed; once | {warmcheck / 60:.0f} min |")
    for name, what, sec in rows:
        print(f"| {name} | {what} | {sec / 60:.0f} min |")
    print(f"| **total** | {a.runs} runs | **{total(warm) / 3600:.1f} h** |")
    if not a.smoke:
        print("\nTotal duration if the warm-up check demands a longer warm-up for every side: "
              + ", ".join(f"{w} s: {total(w) / 3600:.1f} h" for w in (3, 10, 30, 60)) + ".")


if __name__ == "__main__":
    main()
