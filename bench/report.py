#!/usr/bin/env python3
"""Tables of a benchmark result from its raw data (BENCHMARKS §0, and later
the README), so that the documents never drift apart from the measurement.

  bench/report.py <result dir> [--conc 16] [--short]

Reads bm2.json (bm2.py --json) from the result directory written by
run_all.sh and prints Markdown: the framework core (a) and the standard
setups (b) in three labelled versions (standard; same semantics: OpenGrid
touch1, which is what Sanctum does in effect; stricter than Laravel:
touch0). With thesis.json (bm2.py with the logic endpoints) also the
THESIS1 curves, their crossover with the best Octane and the time split per
point (base cost at n = 0 against the part that grows with n); with
load.json, memory.json, coldstart.json the BENCH1 tables (many connections,
fixed rate, memory, cold start). Both sides run on SQLite (D24); the
database overhead is not measured separately.
OpenGrid's PGO build leads and carries the ratios (release default,
PERF4 b); --short leaves out the column without PGO.
"""

import argparse, json, os

ENDPOINTS = ["health", "list", "show", "create", "login"]


def span(rng, key):
    """' [min–max]' of a point's requests/s over the runs of a series, else ''."""
    r = rng.get(key) if rng else None
    return f" [{fmt(r[0])}–{fmt(r[1])}]" if r else ""


def ranges(d, name):
    p = os.path.join(d, name)
    return json.load(open(p)) if os.path.exists(p) else {}


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
    c = a.conc

    rng = ranges(a.dir, "bm2.range.json")

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
            cols += [("OpenGrid + SurrealDB server (without PGO)", f"opengrid-server{suffix}"), ("Laravel Octane (FrankenPHP)", "laravel-frankenphp"), ("Laravel Octane (RoadRunner)", "laravel-octane"), ("Laravel FPM", "laravel-fpm")]
        if not any(get(e, t) for e in endpoints for _, t in cols):
            return
        print(f"**{title}** — {note}\n")
        print("| Endpoint | " + " | ".join(n for n, _ in cols) + " | OpenGrid / best Octane |")
        print("|---|" + "---:|" * (len(cols) + 1))
        for e in endpoints:
            cells = []
            for _, t in cols:
                r = get(e, t)
                cells.append(f"{fmt(r[0])}{span(rng, f'{e}|{t}|{c}')} (p99 {r[2]:.1f} ms)" if r else "–")
            # against the faster of the two Octane servers (Laravel's best case)
            emb = get(e, cols[0][1])
            octanes = [r for r in (get(e, "laravel-frankenphp-core" if core else "laravel-frankenphp"), get(e, "laravel-octane-core" if core else "laravel-octane")) if r]
            best = max(octanes, key=lambda r: r[0]) if octanes else None
            ratio = f"{emb[0] / best[0]:.1f}×" if emb and best else "–"
            print(f"| {e} | " + " | ".join(cells) + f" | {ratio} |")
        print()

    envp = os.path.join(a.dir, "env.txt")
    php = next((l.split("php per target: ", 1)[1].strip() for l in open(envp) if l.startswith("php per target: ")), None) if os.path.exists(envp) else None
    if php:
        # FrankenPHP brings its own, newer PHP: rather an advantage for Laravel
        print(f"PHP: {php}.\n")
    runs = next((l.split()[1] for l in open(envp) if l.startswith("series ") and "runs:" in l), None) if os.path.exists(envp) else None
    if runs:
        print(f"Median of {runs} runs of the same binaries; the range of requests per second over the runs in square brackets. Non-2xx responses are summed over the runs.\n")
    print(f"Requests per second at {c} connections (p99 in brackets); 0 non-2xx responses unless stated.\n")
    section("(a) Framework core: raw SQL on SQLite", "", ENDPOINTS, "both frameworks run exactly the same SQL statements against the same SQLite database in their own process (same library, WAL, synchronous FULL); ORM and auth are replaced by these statements on both sides on purpose", core=True)
    section("(b) Standard setups", "", ENDPOINTS, "both sides in their default settings (OpenGrid notes a token's use at most every 60 s, Sanctum at most once a second)")
    section("(b) Same semantics", "-touch1", ["list", "show", "create"], "OpenGrid notes a token's use at most once a second, as Sanctum does in effect")
    section("(b) Stricter than Laravel", "-touch0", ["list", "show", "create"], "OpenGrid writes the token's use on every request")
    errors = sum(v[3] for v in bm2.values())
    total = sum(v[4] for v in bm2.values())
    print(f"Non-2xx responses: {errors} of {fmt(total)} requests.\n")

    thesis(a.dir)
    bench1(a.dir)


def bench1(d):
    """BENCH1 results (load.py, memory.py, coldstart.py), when present."""
    import statistics
    lp = os.path.join(d, "load.json")
    if os.path.exists(lp):
        load = json.load(open(lp))
        lrng = ranges(d, "load.range.json")
        eps = sorted({k.split("|")[0] for k in load})
        for e in eps:
            rows = [(k.split("|")[1], v) for k, v in load.items() if k.split("|")[0] == e]
            concs = sorted({int(c) for _, v in rows for c in v["closed"]})
            print(f"**(g) Many connections, `{e}`** — closed loop; requests per second (p99 / p99.9 in ms); failures are non-2xx responses and connection errors.\n")
            print("| Target | " + " | ".join(f"{c} connections" for c in concs) + " | failures |")
            print("|---|" + "---:|" * (len(concs) + 1))
            for t, v in rows:
                lr = lrng.get(f"{e}|{t}", {}).get("closed", {})
                cells = [f"{fmt(x['rps'])}" + (f" [{fmt(lr[str(c)][0])}–{fmt(lr[str(c)][1])}]" if str(c) in lr else "") + f" ({x['p99']:.1f} / {x['p999']:.1f})" + (" ‡" if x.get("load_busy", 0) > 90 else "") for c in concs for x in [v["closed"][str(c)]]]
                print(f"| {t} | " + " | ".join(cells) + f" | {sum(v['closed'][str(c)]['bad'] for c in concs)} |")
            levels = sorted(int(x) for x in rows[0][1]["open"])
            print(f"\n**(h) Fixed request rate, `{e}`** — open loop (oha -q with latency correction) at a share of each target's own capacity (its rate at 256 connections); p99 in ms. Knee: the first level whose p99 is more than twice the p99 at {levels[0]} %. Spike: {levels[0]} % plus 65 % for 4 s.\n")
            print("| Target | capacity req/s | " + " | ".join(f"{lv} %" for lv in levels) + " | knee | spike: p99 without → with |")
            print("|---|---:|" + "---:|" * len(levels) + "---|---:|")
            for t, v in rows:
                o = v["open"]
                base = o[str(levels[0])]["p99"]
                knee = next((f"at {lv} %" for lv in levels[1:] if o[str(lv)]["p99"] > 2 * base), f"none up to {levels[-1]} %")
                print(f"| {t} | {fmt(v['capacity'])} | " + " | ".join(f"{o[str(lv)]['p99']:.1f}" for lv in levels) + f" | {knee} | {base:.1f} → {v['spike']['base']['p99']:.1f} |")
            print()
        if any(x.get("load_busy", 0) > 90 for v in load.values() for x in v["closed"].values()):
            print("‡ the load generator's cores were more than 90 % busy: the client may be the limit at this point.\n")
    mp = os.path.join(d, "memory.json")
    if os.path.exists(mp):
        mem = json.load(open(mp))
        mib = lambda k: f"{k / 1024:.0f}"
        print("**(i) Memory** — PSS (shared pages split between the processes sharing them) over every process of a stack, in MiB; *app + database* adds the SurrealDB server; embedded databases (SurrealKV, SQLite) live in the app process.\n")
        print("| Target | app idle | app under load (peak) | app after 10 000 requests | app + database idle | app + database under load (peak) |")
        print("|---|---:|---:|---:|---:|---:|")
        for t, r in mem.items():
            print(f"| {t}{'' if r['db_separate'] else ' (embedded database)'} | {mib(r['idle'][0])} | {mib(r['load_app_peak'])} | {mib(r['after'][0])} | {mib(sum(r['idle']))} | {mib(r['load_all_peak'])} |")
        print()
    cp = os.path.join(d, "coldstart.json")
    if os.path.exists(cp):
        cold = json.load(open(cp))
        print("**(j) Cold start** — from starting the server to the first 200 of an empty route (median of the starts); Laravel with its caches built beforehand (as after a deploy) and without; then the first request of `show` against the median of the following ones (ms).\n")
        print("| Target | workers | Laravel caches | start ms | first request ms | warm ms |")
        print("|---|---:|---|---:|---:|---:|")
        for t, r in cold.items():
            for key, starts in r["start"].items():
                w, caches = key.split("|")
                print(f"| {t} | {w} | {dict(keep='built', none='none').get(caches, '–')} | {statistics.median(starts):.0f} | {statistics.median(r['first'][key]):.1f} | {statistics.median(r['warm'][key]):.2f} |")
        how = {"Dateiwächter": "file watcher", "opcache-Prüfung": "opcache revalidation (revalidate_freq=2)", "octane:reload": "php artisan octane:reload"}
        print("\n| Target | hot reload | to the new answer, ms (median of 3) |")
        print("|---|---|---:|")
        for t, r in cold.items():
            print(f"| {t} | {how.get(r['reload']['how'], r['reload']['how'])} | {statistics.median(r['reload']['ms']):.0f} |")
        print()


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
    the time split per point (base cost at n = 0, the rest grows with n)."""
    path = os.path.join(d, "thesis.json")
    if not os.path.exists(path):
        return
    t = json.load(open(path))
    get = lambda e, tg, c: t.get(f"{e}|{tg}|{c}")
    trng = ranges(d, "thesis.range.json")
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
            cells = [f"{fmt(r[0])}{span(trng, f'{e}|{tg}|16')} (p99 {r[2]:.1f} ms)" if (r := get(e, tg, 16)) else "–" for _, tg in cols]
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
    print("**(f) Time split per point** (1 connection, p50 in ms): *base* = the same endpoint with n = 0 (token check, routing, the query with no rows or none, empty answer; validate: json with n = 0); *grows with n* = the rest: application logic, output and the larger query. Both sides query the same SQLite, so the database part of the growth is the same library on both sides.\n")
    print("| Workload | n | Target | total | base | grows with n |")
    print("|---|---:|---|---:|---:|---:|")
    for w in WORKLOADS:
        base_e = "json:0" if w == "validate" else f"{w}:0"
        for x in NS:
            for label, tg in cols:
                if tg.startswith("opengrid") and tg != lead:
                    continue
                r, b = get(f"{w}:{x}", tg, 1), get(base_e, tg, 1)
                if not r or not b:
                    continue
                print(f"| {w} | {x} | {label} | {r[1]:.2f} | {b[1]:.2f} | {r[1] - b[1]:.2f} |")
    print()

if __name__ == "__main__":
    main()
