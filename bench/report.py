#!/usr/bin/env python3
"""Tables of a benchmark result from its raw data (BENCHMARKS §0, and later
the README), so that the documents never drift apart from the measurement.

  bench/report.py <result dir> [--conc 16] [--short]

Reads bm2.json (bm2.py --json) and, if present, dbbase.json (dbbase.py
--json) and env.txt from the result directory written by run_all.sh, and
prints Markdown: the framework tables in three labelled versions (standard
setups; same semantics: OpenGrid touch1, which is what Sanctum does in
effect; stricter than Laravel: touch0) and the database baseline next to
the total time (BENCHMARKS §0.4). --short leaves out the PGO column.
"""

import argparse, json, os

ENDPOINTS = ["health", "list", "show", "create", "login"]


def fmt(x):
    return f"{x:,.0f}".replace(",", " ")


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
        if core:
            cols = [("OpenGrid", "opengrid-embedded-core")]
            if not a.short:
                cols.append(("with PGO", "opengrid-embedded-pgo-core"))
            cols += [("Laravel Octane (FrankenPHP)", "laravel-frankenphp-core"), ("Laravel Octane (RoadRunner)", "laravel-octane-core"), ("Laravel FPM", "laravel-fpm-core")]
        else:
            cols = [("OpenGrid embedded", f"opengrid-embedded{suffix}")]
            if not a.short:
                cols.append(("with PGO", f"opengrid-embedded-pgo{suffix}"))
            cols += [("OpenGrid + SurrealDB server", f"opengrid-server{suffix}"), ("Laravel Octane (FrankenPHP)", "laravel-frankenphp-pgsql"), ("Laravel Octane (RoadRunner)", "laravel-octane-pgsql"), ("Laravel FPM", "laravel-fpm-pgsql")]
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


if __name__ == "__main__":
    main()
