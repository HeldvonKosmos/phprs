#!/usr/bin/env python3
"""Tables of a benchmark result from its raw data (BENCHMARKS §0, and later
the README), so that the documents never drift apart from the measurement.

  bench/report.py <result dir> [--conc 16] [--short]

Reads bm2.json (bm2.py --json) from the result directory written by
run_all.sh and prints Markdown: the framework core (a), raw SQL on SQLite
on both sides (D25: SurrealDB is not measured, no standard setups). With thesis.json (bm2.py with the logic endpoints) also the
THESIS1 curves, their crossover with the best Octane and the time split per
point (base cost at n = 0 against the part that grows with n); with
load.json, memory.json, coldstart.json the BENCH1 tables (many connections,
fixed rate, memory, cold start). Both sides run on SQLite; the database
overhead is not measured separately.
OpenGrid's PGO build leads and carries the ratios (release default,
PERF4 b); --short leaves out the column without PGO.
"""

import argparse, json, os, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "framework"))
import targets as fw  # noqa: E402

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


def run_dirs(d):
    """The run directories of a result directory (env.txt: "series N runs: …"), else [d]."""
    envp = os.path.join(d, "env.txt")
    line = next((l for l in open(envp) if l.startswith("series ") and "runs:" in l), None) if os.path.exists(envp) else None
    if not line:
        return [d]
    return [os.path.join(d, x) if os.path.isdir(os.path.join(d, x)) else os.path.join(os.path.dirname(os.path.normpath(d)), x) for x in line.split("runs:", 1)[1].split()]


def disturbances(d, bm2):
    """Everything that disturbed or limits the measurement, per run and for
    the whole result, so that it is shown with the numbers (Jonas, 2026-10-28)."""
    import re, collections
    runs = run_dirs(d)
    print("**Disturbances** — recorded during the measurement and shown with the results.\n")
    # non-2xx per side and where
    for pre, (label, _) in fw.FRAMEWORKS.items():
        vals = [(k, v) for k, v in bm2.items() if k.count("|") == 2 and not k.startswith(("answer|", "runtime|")) and fw.framework_of(k.split("|")[1]) == pre]
        if not vals:
            continue
        bad, total = sum(v[3] for _, v in vals), sum(v[4] for _, v in vals)
        where = sorted(f"{k.split('|')[1]} {k.split('|')[0]} at {k.split('|')[2]} connections: {v[3]}" for k, v in vals if v[3])
        note = " — accepted: SQLite's write lock was not free within busy_timeout (5 s) under sustained writes" if pre != "laravel" and bad else ""
        print(f"- Non-2xx responses, {label}: {bad} of {fmt(total)}" + (f" ({'; '.join(where)})" if where else "") + note + ".")
        if pre == "laravel" and bad:
            print("- **Warning: non-2xx responses on the Laravel side; the run is not accepted as it stands.**")
    for i, r in enumerate(runs, 1):
        tag = f"Run {i}" if len(runs) > 1 else "Run"
        q = os.path.join(r, "quiet.txt")
        qtxt = open(q).read().strip() if os.path.exists(q) else "no quiet check"
        kinds = collections.Counter()
        ql = os.path.join(r, "quiet.log")
        if os.path.exists(ql):
            for line in open(ql):
                rest = re.sub(r"^\d\d:\d\d:\d\d ", "", line.strip())
                rest = re.sub(r"process \d+ ", "process ", rest)
                kinds[re.sub(r": \d+ %.*$", "", rest)] += 1
        busy = ""
        lp = os.path.join(r, "load.json")
        if os.path.exists(lp):
            sat = [f"{k} {c}" for k, v in json.load(open(lp)).items() for c, x in v["closed"].items() if x.get("load_busy", 0) > 90]
            busy = f"; load generator above 90 % busy at {len(sat)} points" + (f" ({', '.join(sat)})" if sat else "")
        warns = []
        import glob
        for ep in [os.path.join(r, "bm2.err"), os.path.join(r, "thesis.err")] + sorted(glob.glob(os.path.join(r, "parts", "*.err"))):
            if os.path.exists(ep):
                warns += [l.strip() for l in open(ep) if l.startswith("WARNING")]
        gp = os.path.join(r, "gates.txt")
        gates_bad = [l.strip() for l in open(gp) if "steps passed" not in l and l.strip() and not l.startswith(("#", "run_all: "))] if os.path.exists(gp) else []
        kinds_txt = ", ".join(f"{k} × {n}" for k, n in kinds.most_common(6))
        print(f"- {tag}: quiet check {qtxt.split(' (see')[0].replace('quiet: ', '')}" + (f" ({kinds_txt})" if kinds_txt else "") + busy
              + (f"; {len(warns)} tool warnings ({'; '.join(warns[:3])}{' …' if len(warns) > 3 else ''})" if warns else "; no tool warnings")
              + (f"; **failed gates: {'; '.join(gates_bad)}**" if gates_bad else "") + ".")
    print("- Findings on idle cores without a thread inside the container come from outside it (host kernel threads or host processes, not visible in the container's PID namespace).")
    print("- SurrealDB is not measured; the OpenGrid targets still open their embedded SurrealDB at start (job poller), with no queries per request: it is part of OpenGrid's start time and memory.\n")


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
            # Laravel on Octane with FrankenPHP only (Jonas, 28.10.); RoadRunner and FPM appear only in older results
            cols += [(n, t) for n, t in LARAVEL_CORE if any(get(e, t) for e in endpoints)]
        else:
            cols = og_cols(get, endpoints, "OpenGrid embedded", f"opengrid-embedded-pgo{suffix}", f"opengrid-embedded{suffix}", a.short)
            cols += [("OpenGrid + SurrealDB server (without PGO)", f"opengrid-server{suffix}"), ("Laravel Octane (FrankenPHP)", "laravel-frankenphp"), ("Laravel Octane (RoadRunner)", "laravel-octane"), ("Laravel FPM", "laravel-fpm")]
        if not any(get(e, t) for e in endpoints for _, t in cols):
            return
        print(f"**{title}** — {note}\n")
        others = other_best(bm2)
        print("| Endpoint | " + " | ".join(n for n, _ in cols) + " | OpenGrid / best Octane |" + (" OpenGrid / best other framework |" if others else ""))
        print("|---|" + "---:|" * (len(cols) + 1 + (1 if others else 0)))
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
            extra = ""
            if others:
                cand = [(get(e, t), t) for t in others if get(e, t)]
                if emb and cand:
                    r, t = max(cand, key=lambda x: x[0][0])
                    extra = f" {emb[0] / r[0]:.2f}× ({fw.FRAMEWORKS[fw.framework_of(t)][0]}) |"
                else:
                    extra = " – |"
            print(f"| {e} | " + " | ".join(cells) + f" | {ratio} |" + extra)
        print()

    envp = os.path.join(a.dir, "env.txt")
    php = next((l.split("php per target: ", 1)[1].strip() for l in open(envp) if l.startswith("php per target: ")), None) if os.path.exists(envp) else None
    if php:
        # FrankenPHP brings its own, newer PHP: rather an advantage for Laravel
        print(f"PHP: {php}.\n")
    runs = next((l.split()[1] for l in open(envp) if l.startswith("series ") and "runs:" in l), None) if os.path.exists(envp) else None
    if runs and runs != "1":
        print(f"Median of {runs} runs of the same binaries; the range of requests per second over the runs in square brackets. Non-2xx responses are summed over the runs.\n")
    disturbances(a.dir, bm2)
    print(f"Requests per second at {c} connections (p99 in brackets).\n")
    section("(a) Framework core: raw SQL on SQLite", "", ENDPOINTS, "both frameworks run exactly the same SQL statements against the same SQLite database in their own process (same library, WAL, synchronous FULL); ORM and auth are replaced by these statements on both sides on purpose", core=True)
    # D25: only the framework core is measured (SQLite on both sides); the
    # standard setups with SurrealDB and Eloquent are no longer part of it

    ranking(a.dir, bm2, rng, c)
    thesis(a.dir)
    thesis_all(a.dir)
    bench1(a.dir)
    warmcurve(a.dir)
    runtimes(bm2)
    deviations()


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
        # the German keys are the labels older result files carry
        how = {"Dateiwächter": "file watcher", "opcache-Prüfung": "opcache revalidation (revalidate_freq=2)", "opcache revalidation": "opcache revalidation (revalidate_freq=2)", "octane:reload": "php artisan octane:reload"}
        print("\n| Target | hot reload | to the new answer, ms (median of 3) |")
        print("|---|---|---:|")
        for t, r in cold.items():
            if r.get("reload"):
                print(f"| {t} | {how.get(r['reload']['how'], r['reload']['how'])} | {statistics.median(r['reload']['ms']):.0f} |")
        print()
    hp = os.path.join(d, "hotreload.json")
    if os.path.exists(hp):
        hot = json.load(open(hp))
        print("**(k) Hot reload in each framework's development mode** (BENCH3) — **development mode, not the production setup measured above**; the empty route's answer changed in the source, time until the server answers with the new code (polled every 20 ms), median (min–max) of all changes of all runs.\n")
        print("| Framework | development mode | to the new answer, ms | without new answer | dev server ready ms |")
        print("|---|---|---:|---:|---:|")
        for k, r in hot.items():
            app = k.split("|")[1]
            t = r["ms"]
            cell = f"{statistics.median(t):.0f} ({min(t):.0f}–{max(t):.0f})" if t else "–"
            boot = r["boot_ms"] if isinstance(r["boot_ms"], (int, float)) else statistics.median(r["boot_ms"])
            print(f"| {fw.FRAMEWORKS[app][0]} | {r['how']} | {cell} | {r['failed']} | {boot:.0f} |")
        print()


def other_best(bm2):
    """The fastest variant (over all of bm2) of every framework other than OpenGrid and Laravel."""
    return [fw.fastest(bm2, f) for f in fw.FRAMEWORKS if f not in ("opengrid", "laravel") and any(fw.score(bm2, t) for t, *_ in fw.FRAMEWORKS[f][1])]


def lead_og(get_any):
    return "opengrid-embedded-pgo-core" if get_any("opengrid-embedded-pgo-core") else "opengrid-embedded-core"


def mark(t):
    """Label of a target with the marks of BENCH3."""
    f = fw.framework_of(t)
    return fw.label(t) + (" — JIT reference" if f == fw.JIT_REFERENCE else "")


def ranking(d, bm2, rng, c):
    """(a') every framework per endpoint (BENCH3): its fastest variant (over
    all endpoints of bm2), ranked by requests/s; Laravel with its best Octane
    server; the other variants in their own table."""
    get = lambda e, t: bm2.get(f"{e}|{t}|{c}")
    if not other_best(bm2):
        return
    og = lead_og(lambda t: any(get(e, t) for e in ENDPOINTS))
    lv = max((t for _, t in LARAVEL_CORE[:2] if fw.score(bm2, t)), key=lambda t: fw.score(bm2, t), default="laravel-frankenphp-core")
    cands = [og, lv] + other_best(bm2)
    print(f"**(a') All frameworks per endpoint** (BENCH3) — raw SQL on SQLite as (a), {c} connections; per framework its fastest server variant over all endpoints (Laravel: the faster Octane server); rank by requests per second; *OpenGrid / it* is {fw.label(og)} against that framework. Spring Boot is the upper reference for a mature JIT (the JVM).\n")
    for e in ENDPOINTS:
        rows = sorted([(get(e, t), t) for t in cands if get(e, t)], key=lambda x: -x[0][0])
        if not rows:
            continue
        ogr = get(e, og)
        print(f"*{e}*\n")
        print("| Rank | Framework (variant) | requests/s | p99 ms | OpenGrid / it |")
        print("|---:|---|---:|---:|---:|")
        for i, (r, t) in enumerate(rows, 1):
            print(f"| {i} | {mark(t)} | {fmt(r[0])}{span(rng, f'{e}|{t}|{c}')} | {r[2]:.1f} | {'–' if t == og or not ogr else f'{ogr[0] / r[0]:.2f}×'} |")
        print()
    multi = [f for f in fw.FRAMEWORKS if f not in ("opengrid", "laravel") and len(fw.FRAMEWORKS[f][1]) > 1]
    if multi:
        print(f"**Server variants** — requests per second at {c} connections per variant (geometric mean over the endpoints in the last column); the faster one (bold) is compared above and measured in (e)–(j).\n")
        print("| Framework | variant | " + " | ".join(ENDPOINTS) + " | geometric mean |")
        print("|---|---|" + "---:|" * (len(ENDPOINTS) + 1))
        for f in multi:
            best = fw.fastest(bm2, f)
            for t, v, *_ in fw.FRAMEWORKS[f][1]:
                name = f"**{v}**" if t == best else v
                print(f"| {fw.FRAMEWORKS[f][0]} | {name} | " + " | ".join(fmt(get(e, t)[0]) if get(e, t) else "–" for e in ENDPOINTS) + f" | {fmt(fw.score(bm2, t))} |")
        print()


CURVE_MARKS = [1, 2, 3, 5, 10, 20, 30, 45, 60]
# one colour per framework, in the order of targets.FRAMEWORKS
CURVE_COLORS = ["#d62728", "#7f7f7f", "#1f77b4", "#17becf", "#bcbd22", "#2ca02c", "#ff7f0e", "#9467bd", "#8c564b", "#e377c2"]


def curve_svg(path, title, series):
    """A line chart of requests/s as a share of each target's steady rate over time."""
    w, h, l, r, t, b = 760, 380, 56, 210, 34, 40
    tmax = max(len(pts) for _, pts, _ in series)
    ymax = max(110, max((p for _, pts, _ in series for p in pts), default=100))
    X = lambda i: l + (w - l - r) * i / max(1, tmax)
    Y = lambda v: t + (h - t - b) * (1 - v / ymax)
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" font-family="sans-serif" font-size="12">',
           f'<rect width="{w}" height="{h}" fill="#ffffff"/>', f'<text x="{l}" y="20" font-size="14" font-weight="bold">{title}</text>']
    for v in range(0, int(ymax) + 1, 20):
        out.append(f'<line x1="{l}" x2="{w - r}" y1="{Y(v):.1f}" y2="{Y(v):.1f}" stroke="{"#999" if v == 100 else "#e5e5e5"}"/>')
        out.append(f'<text x="{l - 6}" y="{Y(v) + 4:.1f}" text-anchor="end" fill="#555">{v} %</text>')
    for sec in range(0, tmax + 1, 10):
        out.append(f'<text x="{X(sec):.1f}" y="{h - b + 16}" text-anchor="middle" fill="#555">{sec} s</text>')
    for k, (name, pts, color) in enumerate(series):
        d = " ".join(f"{X(i + 1):.1f},{Y(v):.1f}" for i, v in enumerate(pts))
        out.append(f'<polyline fill="none" stroke="{color}" stroke-width="2" points="{d}"/>')
        out.append(f'<line x1="{w - r + 14}" x2="{w - r + 34}" y1="{t + 8 + k * 18}" y2="{t + 8 + k * 18}" stroke="{color}" stroke-width="3"/>')
        out.append(f'<text x="{w - r + 40}" y="{t + 12 + k * 18}">{name}</text>')
    out.append("</svg>")
    open(path, "w").write("\n".join(out))


def warmcurve(d):
    """(l) Warm-up curves (warmcurve.py): requests/s over time after a cold start."""
    p = os.path.join(d, "warmcurve.json")
    if not os.path.exists(p):
        return
    cv = json.load(open(p))
    order = list(fw.FRAMEWORKS)
    tgs = sorted({k.split("|")[1] for k in cv}, key=lambda x: order.index(fw.framework_of(x) or "opengrid"))
    for e in sorted({k.split("|")[0] for k in cv}):
        rows = [(x, cv[f"{e}|{x}"]) for x in tgs if f"{e}|{x}" in cv]
        n = min(len(r["points"]) for _, r in rows)
        marks = [m for m in CURVE_MARKS if m <= n]
        print(f"**(l) Warm-up curve, `{e}`** (BENCH3) — a freshly started server under constant load (16 connections, no warm-up); requests per second in each second as a share of the target's steady rate (median of the last 10 s), so a runtime that optimises while it runs (JIT) starts low and climbs; *warm from* = the first second from which the 3-s mean no longer falls below 90 % / 97 % of the steady rate. Spring Boot is the JIT reference.\n")
        svg = f"warmcurve-{e.replace(':', '-')}.svg"
        colors = {f: CURVE_COLORS[i % len(CURVE_COLORS)] for i, f in enumerate(order)}
        curve_svg(os.path.join(d, svg), f"Warm-up after a cold start: {e} (share of steady requests/s)",
                  [(fw.label(x), [100 * pt["rps"] / r["steady"] if r["steady"] else 0 for pt in r["points"]], colors[fw.framework_of(x) or "opengrid"]) for x, r in rows])
        print(f"![Warm-up curve {e}]({svg})\n")
        print("| Target | " + " | ".join(f"{m} s" for m in marks) + " | steady req/s | warm 90 % from | warm 97 % from |")
        print("|---|" + "---:|" * (len(marks) + 3))
        for x, r in rows:
            share = lambda m: f"{100 * r['points'][m - 1]['rps'] / r['steady']:.0f} %" if r["steady"] else "–"
            ws = lambda v: "–" if v is None else f"{v:g} s"
            rng_s = f" [{fmt(r['range_steady'][0])}–{fmt(r['range_steady'][1])}]" if "range_steady" in r else ""
            print(f"| {mark(x)} | " + " | ".join(share(m) for m in marks) + f" | {fmt(r['steady'])}{rng_s} | {ws(r['warm90'])} | {ws(r['warm97'])} |")
        print()


def runtimes(bm2):
    rt = {k.split("|", 1)[1]: v for k, v in bm2.items() if k.startswith("runtime|")}
    if not rt:
        return
    print("**Runtimes per target** (BENCH3) — as each side reports them (`/api/bench/sql/sqlite`); every side checked against the reference SQLite 3.53.4 and its compile options before measuring.\n")
    print("| Target | SQLite | runtime |")
    print("|---|---|---|")
    for t, r in rt.items():
        print(f"| {mark(t)} | {r.get('sqlite', '?')} | {', '.join(f'{k} {v}' for k, v in r.items() if k != 'sqlite')} |")
    print()


def deviations():
    """The deviations of the comparison apps and the validate rule table (bench/frameworks/DEVIATIONS.md)."""
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)), "frameworks", "DEVIATIONS.md")
    if os.path.exists(p):
        text = open(p).read()
        # without the file's own title
        print(text.split("\n", 1)[1].strip() + "\n" if text.startswith("# ") else text)


def thesis_all(d):
    """THESIS1 for every framework (BENCH3): requests/s at 16 connections
    per workload and n, and the fixed and per-element cost (G, k) of the
    model total time = G + n × k, fitted to the 1-connection p50."""
    path = os.path.join(d, "thesis.json")
    if not os.path.exists(path):
        return
    t = json.load(open(path))
    tg = sorted({k.split("|")[1] for k in t if k.count("|") == 2 and not k.startswith(("answer|", "runtime|"))}, key=lambda x: list(fw.FRAMEWORKS).index(fw.framework_of(x) or "opengrid"))
    if not any(fw.framework_of(x) not in ("opengrid", "laravel") for x in tg):
        return
    get = lambda e, x, c: t.get(f"{e}|{x}|{c}")
    c1 = 1 if any(k.endswith("|1") for k in t) else 16
    print("**(e') Logic per request, all frameworks** (BENCH3) — as (e), requests per second at 16 connections; per framework its fastest variant from (a).\n")
    for w in WORKLOADS:
        print(f"*{w}*\n")
        print("| Target | " + " | ".join(f"n = {n}" for n in NS) + " |")
        print("|---|" + "---:|" * len(NS))
        for x in tg:
            print(f"| {mark(x)} | " + " | ".join(fmt(r[0]) if (r := get(f"{w}:{n}", x, 16)) else "–" for n in NS) + " |")
        print()
    print(f"**(f') Fixed and per-element cost** — total time per request = G + n × k, a least-squares line through the p50 at {c1} connection{'s' if c1 > 1 else ''} for n = 0 (validate: json with n = 0) and every measured n; G in ms, k in µs per element.\n")
    print("| Target | " + " | ".join(f"{w} G | {w} k" for w in WORKLOADS) + " |")
    print("|---|" + "---:|---:|" * len(WORKLOADS))
    for x in tg:
        cells = []
        for w in WORKLOADS:
            pts = [(0, b[1])] if (b := get("json:0" if w == "validate" else f"{w}:0", x, c1)) else []
            pts += [(n, r[1]) for n in NS if (r := get(f"{w}:{n}", x, c1))]
            if len(pts) < 2:
                cells += ["–", "–"]
                continue
            mx = sum(p[0] for p in pts) / len(pts)
            my = sum(p[1] for p in pts) / len(pts)
            k = sum((p[0] - mx) * (p[1] - my) for p in pts) / sum((p[0] - mx) ** 2 for p in pts)
            cells += [f"{my - k * mx:.3f}", f"{k * 1000:.2f}"]
        print(f"| {mark(x)} | " + " | ".join(cells) + " |")
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
    # the connection count of the time split: 1 when measured (older runs), else 16 (compact plan, 28.10.)
    c1 = 1 if any(k.endswith("|1") for k in t) else 16
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
    print(f"**(f) Time split per point** ({c1} connection{'s' if c1 > 1 else ''}, p50 in ms): *base* = the same endpoint with n = 0 (token check, routing, the query with no rows or none, empty answer; validate: json with n = 0); *grows with n* = the rest: application logic, output and the larger query. Both sides query the same SQLite, so the database part of the growth is the same library on both sides.\n")
    print("| Workload | n | Target | total | base | grows with n |")
    print("|---|---:|---|---:|---:|---:|")
    for w in WORKLOADS:
        base_e = "json:0" if w == "validate" else f"{w}:0"
        for x in NS:
            for label, tg in cols:
                if tg.startswith("opengrid") and tg != lead:
                    continue
                r, b = get(f"{w}:{x}", tg, c1), get(base_e, tg, c1)
                if not r or not b:
                    continue
                print(f"| {w} | {x} | {label} | {r[1]:.2f} | {b[1]:.2f} | {r[1] - b[1]:.2f} |")
    print()

if __name__ == "__main__":
    main()
