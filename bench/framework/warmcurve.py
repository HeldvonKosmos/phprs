#!/usr/bin/env python3
"""Warm-up curve (BENCH3, Jonas 28.10.): how a runtime gets faster under
load after a cold start, per target set up as by bm2.py. Per target and
endpoint: a freshly started server (start, seed, one login: bm2.py's
setup), then --duration seconds of constant closed-loop load at --conc
connections with no warm-up; oha records every finished request (CSV:
start and duration), which are counted per --bin seconds by their end time.

Per point of the curve: requests/s and the p50/p99 latency of the requests
finished in that bin. Derived: the steady rate (median of the last
--steady seconds), the first second as a share of it, and the time to warm:
the first bin from which the mean of every --smooth consecutive bins stays
at or above 90 % and 97 % of the steady rate (JIT tiers, caches, connection
pools all count; the restart itself is in coldstart.py).

  warmcurve.py [--targets …] [--endpoints list,hydrate:100] [--duration 60]
               [--conc 16] [--bin 1] [--steady 10] [--smooth 3] [--json FILE]
"""

import argparse, csv, json, os, statistics, subprocess, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402

MARKS = [1, 2, 3, 5, 10, 20, 30, 45, 60]


def curve(url, a, method, body, token):
    """Requests per bin and their p50/p99 (ms), from oha's per-request CSV."""
    cmd = ["taskset", "-c", a.load_cores, "oha", "--no-tui", "--output-format", "csv", "-z", f"{a.duration}s", "-c", str(a.conc), "-m", method]
    if token:
        cmd += ["-H", f"Authorization: Bearer {token}"]
    if body is not None:
        cmd += ["-T", "application/json", "-d", json.dumps(body)]
    p = subprocess.Popen(cmd + [url], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    n = int(a.duration / a.bin)
    lat = [[] for _ in range(n)]
    bad = 0
    reader = csv.reader(p.stdout)
    next(reader, None)
    for row in reader:
        start, duration, status = float(row[0]), float(row[4]), row[6]
        if not status.startswith("2"):
            bad += 1
            continue
        i = int((start + duration) / a.bin)
        if 0 <= i < n:
            lat[i].append(duration * 1000)
    p.wait()
    if p.returncode != 0:
        sys.exit(f"oha failed: {p.stderr.read()[-500:]}")
    points = []
    for xs in lat:
        xs.sort()
        q = lambda f: xs[min(len(xs) - 1, int(f * len(xs)))] if xs else None
        points.append({"rps": len(xs) / a.bin, "p50": q(0.5), "p99": q(0.99)})
    return points, bad


def derive(points, a):
    rps = [x["rps"] for x in points]
    k = max(1, int(a.steady / a.bin))
    steady = statistics.median(rps[-k:])
    m = max(1, int(a.smooth / a.bin))
    smooth = [sum(rps[i:i + m]) / m for i in range(len(rps) - m + 1)]

    def warm_at(share):
        for i in range(len(smooth)):
            if all(v >= share * steady for v in smooth[i:]):
                return round(i * a.bin, 3)
        return None
    return {"steady": steady, "first": rps[0] / steady if steady else 0, "warm90": warm_at(0.90), "warm97": warm_at(0.97)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default="opengrid-embedded-pgo-core,laravel-frankenphp-core")
    ap.add_argument("--endpoints", default="list,hydrate:100")
    ap.add_argument("--duration", type=int, default=60)
    ap.add_argument("--conc", type=int, default=16)
    ap.add_argument("--bin", type=float, default=1.0)
    ap.add_argument("--steady", type=float, default=10.0)
    ap.add_argument("--smooth", type=float, default=3.0)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--notes", type=int, default=1000)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/perf/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    a.app_cores, a.db_cores, a.load_cores = bm2.APP_CORES, bm2.DB_CORES, bm2.LOAD_CORES
    a.opengrid, a.opengrid_pgo = os.path.abspath(a.opengrid), os.path.abspath(a.opengrid_pgo)
    res = {}
    for name in a.targets.split(","):
        for e in a.endpoints.split(","):
            with tempfile.TemporaryDirectory(prefix=f"curve-{name}-") as work:
                t = bm2.Target(name, a, work)
                try:
                    t.launch()
                    t.seed()
                    url, method, body, token = t.endpoint(e)
                    points, bad = curve(url, a, method, body, token)
                finally:
                    t.close()
            d = derive(points, a)
            res[f"{e}|{name}"] = {"points": points, "bad": bad, **d}
            print(f"  {name:<28} {e:<12} steady {d['steady']:8.0f} req/s  first {d['first'] * 100:5.1f} %  warm 90 % at {d['warm90']} s, 97 % at {d['warm97']} s  non-2xx {bad}", file=sys.stderr)
    marks = [m for m in MARKS if m <= a.duration]
    print(f"Warm-up curve (BENCH3) · per target and endpoint a freshly started server, {a.duration} s of constant load at {a.conc} connections without warm-up, "
          f"requests counted per {a.bin:g} s by their end · steady rate = median of the last {a.steady:g} s · warm from = the first second from which the moving "
          f"{a.smooth:g}-s mean no longer falls below 90 % or 97 % of the steady rate\n")
    for e in a.endpoints.split(","):
        print(f"#### {e}\n")
        print("| Target | " + " | ".join(f"{m} s" for m in marks) + " | Steady rate | 1st s / steady | Warm 90 % from | Warm 97 % from |")
        print("|---|" + "---:|" * (len(marks) + 4))
        for name in a.targets.split(","):
            r = res[f"{e}|{name}"]
            cells = [f"{r['points'][int(m / a.bin) - 1]['rps']:.0f}" for m in marks]
            fmt_s = lambda v: "–" if v is None else f"{v:g} s"
            print(f"| {name} | " + " | ".join(cells) + f" | {r['steady']:.0f} | {r['first'] * 100:.0f} % | {fmt_s(r['warm90'])} | {fmt_s(r['warm97'])} |")
        print()
    if a.json:
        json.dump(res, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
