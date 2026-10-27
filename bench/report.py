#!/usr/bin/env python3
"""Tables of a benchmark result from its raw data (BENCHMARKS §0, and later
the README), so that the documents never drift apart from the measurement.

  bench/report.py <result dir> [--conc 16] [--short]

Reads bm2.json (bm2.py --json) and, if present, dbbase.json (dbbase.py
--json) and env.txt from the result directory written by run_all.sh, and
prints Markdown: the framework tables in three labelled versions (standard
setups; same semantics: OpenGrid touch1, which is what Sanctum does in
effect; stricter than Laravel: touch0) and the database baseline next to
the total time (BENCHMARKS §0.4). With thesis.json (bm2.py with the logic
endpoints) and sqlbase.json (sqlite/sqlbase.py) also the THESIS1 curves,
their crossover with the best Octane and the time split per point.
OpenGrid's PGO build leads and carries the ratios (release default,
PERF4 b); --short leaves out the column without PGO.
"""

import argparse, json, os

ENDPOINTS = ["health", "list", "show", "create", "login"]


def fmt(x):
    return f"{x:,.0f}".replace(",", " ")


def og_cols(get, endpoints, label, pgo, plain, short):
    """OpenGrid's columns: the PGO build first (release default, PERF4 b),
    then the build without PGO; only the plain one when no PGO run exists."""
    if any(get(e, pgo) for e in endpoints):
        return [(f"{label} (PGO)", pgo)] + ([] if short else [("without PGO", plain)])
    return [(f"{label} (without PGO)", plain)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--conc", type=int, default=16)
    ap.add_argument("--short", action="store_true")
    a = ap.parse_args()
    bm2 = json.load(open(os.path.join(a.dir, "bm2.json")))
    base_path = os.path.join(a.dir, "dbbase.json")
    base = json.load(open(base_path)) if os.path.exists(base_path) else {}
    c = a.conc

    def get(e, t, conc=c):
        return bm2.get(f"{e}|{t}|{conc}")

    def section(title, suffix, endpoints, note, core=False):
        # PERF4 b: PGO is the release default, so the PGO binary leads and
        # carries the ratio; the build without PGO keeps its own column
        if core:
            cols = og_cols(get, endpoints, "OpenGrid", "opengrid-embedded-pgo-core", "opengrid-embedded-core", a.short)
            cols += [("Laravel Octane (FrankenPHP)", "laravel-frankenphp-core"), ("Laravel Octane (RoadRunner)", "laravel-octane-core"), ("Laravel FPM", "laravel-fpm-core")]
        else:
            cols = og_cols(get, endpoints, "OpenGrid embedded", f"opengrid-embedded-pgo{suffix}", f"opengrid-embedded{suffix}", a.short)
            cols += [("OpenGrid + SurrealDB server (without PGO)", f"opengrid-server{suffix}"), ("Laravel Octane (FrankenPHP)", "laravel-frankenphp-pgsql"), ("Laravel Octane (RoadRunner)", "laravel-octane-pgsql"), ("Laravel FPM", "laravel-fpm-pgsql")]
        if not any(get(e, t) for e in endpoints for _, t in cols):
            return
        print(f"**{title}** — {note}\n")
        print("| Endpoint | " + " | ".join(n for n, _ in cols) + " | OpenGrid / best Octane |")
        print("|---|" + "---:|" * (len(cols) + 1))
        for e in endpoints:
            cells = []
            for _, t in cols:
                r = get(e, t)
                cells.append(f"{fmt(r[0])} (p99 {r[2]:.1f} ms)" if r else "–")
            # against the faster of the two Octane servers (Laravel's best case)
            emb = get(e, cols[0][1])
            octanes = [r for r in (get(e, "laravel-frankenphp-core" if core else "laravel-frankenphp-pgsql"), get(e, "laravel-octane-core" if core else "laravel-octane-pgsql")) if r]
            best = max(octanes, key=lambda r: r[0]) if octanes else None
            ratio = f"{emb[0] / best[0]:.1f}×" if emb and best else "–"
            print(f"| {e} | " + " | ".join(cells) + f" | {ratio} |")
        print()

    print(f"Requests per second at {c} connections (p99 in brackets); 0 non-2xx responses unless stated.\n")
    section("(a) Framework core: raw SQL on SQLite", "", ENDPOINTS, "both frameworks run exactly the same SQL statements against the same SQLite database in their own process (same library, WAL, synchronous FULL); ORM and auth are replaced by these statements on both sides on purpose", core=True)
    section("(b) Standard setups", "", ENDPOINTS, "both sides in their default settings (OpenGrid notes a token's use at most every 60 s, Sanctum at most once a second)")
    section("(b) Same semantics", "-touch1", ["list", "show", "create"], "OpenGrid notes a token's use at most once a second, as Sanctum does in effect")
    section("(b) Stricter than Laravel", "-touch0", ["list", "show", "create"], "OpenGrid writes the token's use on every request")
    errors = sum(v[3] for v in bm2.values())
    total = sum(v[4] for v in bm2.values())
    print(f"Non-2xx responses: {errors} of {fmt(total)} requests.\n")

    if base:
        print("**(c) Database share** (1 connection, p50 in ms): total time of the request in BM2, time of the same statements against the same database without the framework (`dbbase.py`), and the difference. The database time was measured without load from framework code, so the difference is an approximation, not an exact value.\n")
        # the baseline writes the token use as often as the target does: never
        # within the run (OpenGrid default, 60 s), once a second (touch1,
        # Laravel/Sanctum), on every request (touch0)
        rows = [
            ("OpenGrid embedded", "opengrid-embedded", "opengrid-embedded", ""),
            ("OpenGrid embedded, touch1", "opengrid-embedded-touch1", "opengrid-embedded", "+touch1"),
            ("OpenGrid embedded, touch0", "opengrid-embedded-touch0", "opengrid-embedded", "+touch"),
            ("OpenGrid + server", "opengrid-server", "opengrid-server", ""),
            ("OpenGrid + server, touch1", "opengrid-server-touch1", "opengrid-server", "+touch1"),
            ("OpenGrid + server, touch0", "opengrid-server-touch0", "opengrid-server", "+touch"),
            ("Laravel Octane", "laravel-octane-pgsql", "laravel-octane-pgsql", "+touch1"),
            ("Laravel FPM", "laravel-fpm-pgsql", "laravel-fpm-pgsql", "+touch1"),
        ]
        print("| Target | Endpoint | total p50 | database p50 | difference (framework) | database share |")
        print("|---|---|---:|---:|---:|---:|")
        for label, t, bt, v in rows:
            for e in ["health", "list", "show", "create", "login"]:
                r = get(e, t, 1)
                if not r:
                    continue
                key = e + (v if e in ("list", "show", "create") else "")
                b = base.get(f"{key}|{bt}|1")
                if not b and e != "health":
                    continue
                db = b["p50_ms"] if b else 0.0
                share = f"{100 * db / r[1]:.0f} %" if r[1] else "–"
                print(f"| {label} | {e} | {r[1]:.2f} | {db:.2f} | {r[1] - db:.2f} | {share} |")
        print("\nThe baseline writes the token's use as often as the target does: not within the run for OpenGrid's default (at most every 60 s), at most once a second for touch1 and Laravel (Sanctum in effect), on every request for touch0; every write sets a new value on both sides.\n")

    thesis(a.dir)


WORKLOADS = ["hydrate", "collect", "validate", "json"]
NS = [1, 10, 100, 1000]
LARAVEL_CORE = [("Laravel Octane (FrankenPHP)", "laravel-frankenphp-core"), ("Laravel Octane (RoadRunner)", "laravel-octane-core"), ("Laravel FPM", "laravel-fpm-core")]


def crossover(points):
    """n where OpenGrid / best Octane falls through 1, interpolated in log10(n)
    between neighbouring points; None when it stays above 1."""
    import math
    if points and points[0][1] < 1:
        return f"at or below n = {points[0][0]} (OpenGrid behind from the first point)"
    for (n0, r0), (n1, r1) in zip(points, points[1:]):
        if r0 >= 1 > r1:
            x = math.log10(n0) + (math.log10(n1) - math.log10(n0)) * (r0 - 1) / (r0 - r1)
            return f"n ≈ {10 ** x:.0f} (between {n0} and {n1})"
    return None


def thesis(d):
    """THESIS1 curves (thesis.json from bm2.py with the logic endpoints) and
    the time split per point (sqlbase.json: the database alone)."""
    path = os.path.join(d, "thesis.json")
    if not os.path.exists(path):
        return
    t = json.load(open(path))
    sp = os.path.join(d, "sqlbase.json")
    sb = json.load(open(sp)) if os.path.exists(sp) else {}
    get = lambda e, tg, c: t.get(f"{e}|{tg}|{c}")
    eps = [f"{w}:{x}" for w in WORKLOADS for x in NS]
    cols = og_cols(lambda e, tg: get(e, tg, 16), eps, "OpenGrid", "opengrid-embedded-pgo-core", "opengrid-embedded-core", False)
    lead = cols[0][1]
    cols += [(n, tg) for n, tg in LARAVEL_CORE if any(get(e, tg, 16) for e in eps)]
    print(f"**(e) Logic per request (THESIS1)** — framework core (raw SQL on SQLite, as (a)), the same workload in both apps with n elements; requests per second at 16 connections (p99 in brackets). The ratio is {cols[0][0]} against the faster Octane server.\n")
    summary = []
    for w in WORKLOADS:
        if not any(get(f"{w}:{x}", lead, 16) for x in NS):
            continue
        print(f"*{w}*\n")
        print("| n | " + " | ".join(n for n, _ in cols) + " | OpenGrid / best Octane |")
        print("|---:|" + "---:|" * (len(cols) + 1))
        points = []
        for x in NS:
            e = f"{w}:{x}"
            cells = [f"{fmt(r[0])} (p99 {r[2]:.1f} ms)" if (r := get(e, tg, 16)) else "–" for _, tg in cols]
            og = get(e, lead, 16)
            octanes = [r for r in (get(e, "laravel-frankenphp-core", 16), get(e, "laravel-octane-core", 16)) if r]
            ratio = og[0] / max(r[0] for r in octanes) if og and octanes else None
            if ratio is not None:
                points.append((x, ratio))
            print(f"| {x} | " + " | ".join(cells) + f" | {f'{ratio:.2f}×' if ratio is not None else '–'} |")
        cross = crossover(points)
        summary.append((w, cross))
        print(f"\nCrossover with the best Octane: {cross or 'none up to n = 1000 (OpenGrid ahead at every point)'}.\n")
    if summary:
        print("**Crossover per workload:** " + "; ".join(f"{w}: {c or 'none up to 1000'}" for w, c in summary) + ".\n")
    if not sb:
        return
    print("**(f) Time split per point** (1 connection, p50 in ms): *database* = the same statements alone (`sqlbase.py`, PDO for Laravel, rusqlite for OpenGrid, same SQLite); *base* = the same endpoint with n = 0 (token check, routing, empty answer) minus its database time (validate: json with n = 0); *logic and output* = the rest. An approximation, as in (c).\n")
    print("| Workload | n | Target | total | database | base | logic and output |")
    print("|---|---:|---|---:|---:|---:|---:|")
    for w in WORKLOADS:
        base_e = "json:0" if w == "validate" else f"{w}:0"
        for x in NS:
            for label, tg in cols:
                if tg.startswith("opengrid") and tg != lead:
                    continue
                r, b = get(f"{w}:{x}", tg, 1), get(base_e, tg, 1)
                if not r or not b:
                    continue
                side = "rust" if tg.startswith("opengrid") else "php"
                db_e = "json:0" if w in ("validate", "json") else f"hydrate:{x}"
                db_b = "json:0" if w in ("validate", "json") else "hydrate:0"
                db = sb.get(f"{db_e}|{side}", {}).get("p50_ms")
                dbb = sb.get(f"{db_b}|{side}", {}).get("p50_ms")
                if db is None or dbb is None:
                    continue
                base = b[1] - dbb
                print(f"| {w} | {x} | {label} | {r[1]:.2f} | {db:.2f} | {base:.2f} | {r[1] - db - base:.2f} |")
    print()


if __name__ == "__main__":
    main()
