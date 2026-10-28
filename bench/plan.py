#!/usr/bin/env python3
"""Plan of `bench/run_all.sh` (BENCH2): every stage, what it measures and
its estimated duration, from the same parameters the stages use (keep in
sync with run_all.sh; `stages.txt` of a run has the real times).

  bench/plan.py [--runs 3] [--framework-only] [--smoke]
Estimates: measured time per point (warm-up + duration) is exact; setup
per target and the builds are taken from earlier runs (77d4cc5: BM2 with
11 targets 36 min; builds of 27.10.).
"""

import argparse, os, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "framework"))
import bm2  # noqa: E402

BM2_TARGETS = "opengrid-embedded-core,opengrid-embedded-pgo-core,laravel-frankenphp-core,laravel-octane-core,laravel-fpm-core".split(",")  # D25
THESIS_TARGETS = 5
THESIS_ENDPOINTS = 3 + 4 * 4
SETUP_S = 15  # start, schema, seed per target (earlier runs: 10-20 s)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--framework-only", action="store_true")
    ap.add_argument("--smoke", action="store_true")
    ap.add_argument("--runs", type=int, default=3)
    a = ap.parse_args()
    point = 2 if a.smoke else 3 + 10  # warm-up + measurement of one point (bm2, load)
    rows = []
    rows.append(("build", "Binaries aus HEAD: profiling grd/opengrid, PGO grd und opengrid (bench/pgo.sh)", 20 * 60 if a.framework_only else 35 * 60))
    rows.append(("gates", "api.sh: OpenGrid ohne und mit PGO; Laravel FPM, RoadRunner, FrankenPHP", 5 * 25))
    if not a.framework_only:
        rows.append(("lang", "Sprache c1–c3, Vergleiche A/B/C, mit PGO, " + ("1 Lauf" if a.smoke else "Median aus 7"), 40 if a.smoke else 180))
    n = len(BM2_TARGETS) * len(bm2.ENDPOINTS) * 3
    rows.append(("bm2", f"BM2: {len(BM2_TARGETS)} Ziele × {len(bm2.ENDPOINTS)} Endpunkte × 1/16/64 Verbindungen = {n} Punkte", n * point + len(BM2_TARGETS) * SETUP_S))
    n = THESIS_TARGETS * THESIS_ENDPOINTS * 2
    rows.append(("thesis", f"THESIS1: {THESIS_TARGETS} Kern-Ziele × {THESIS_ENDPOINTS} Logik-Endpunkte × 1/16 Verbindungen = {n} Punkte", n * point + THESIS_TARGETS * 20))
    lt, le = 4, 2
    per = (3 * point + 4 * point + (6 if a.smoke else 19))
    rows.append(("load", f"Gleichzeitigkeit/offene Last: {lt} Ziele × {le} Endpunkte × (3 geschlossen + 4 Stufen + Spitze)", lt * le * per + lt * SETUP_S))
    rows.append(("memory", "Speicher PSS: 4 Ziele (Ruhe, Last, nach 10 000 Anfragen)", 4 * (SETUP_S + (8 if a.smoke else 25))))
    runs = 1 if a.smoke else 5
    rows.append(("coldstart", "Kaltstart: 4 Kern-Ziele, 4/16 Worker, Laravel mit/ohne Caches, erste Anfrage, Hot-Reload", 4 * SETUP_S + (2 + 3 * 4) * runs * 3 + 4 * 10))
    rows.append(("report", "Tabellen aus den Rohdaten (report.py)", 5))
    # build once, every measuring stage once per run
    total = sum(r[2] if r[0] == "build" else r[2] * a.runs for r in rows)
    print(f"Plan von bench/run_all.sh --runs {a.runs}{' --framework-only' if a.framework_only else ''}{' --smoke' if a.smoke else ''} (Schätzung; Build einmal, jede Stufe je Lauf)\n")
    print("| Stufe | Inhalt | geschätzt je Lauf |")
    print("|---|---|---:|")
    for name, what, sec in rows:
        print(f"| {name} | {what} | {sec / 60:.0f} min |")
    print(f"| **gesamt** | | **{total / 3600:.1f} h** |")


if __name__ == "__main__":
    main()
