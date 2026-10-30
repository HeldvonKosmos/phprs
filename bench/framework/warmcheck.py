#!/usr/bin/env python3
"""Warm-up check (BENCH3): is the warm-up of bm2.py long enough for every
side? Per target, set up as by bm2.py, and per endpoint: a fresh server,
--warmup seconds of load, then three consecutive windows of --window
seconds at --conc connections; the warm-up suffices when the requests/s of
the three windows differ by at most --tolerance (max / min - 1). If not,
the next longer warm-up of --steps is tried on a fresh server. The longest
warm-up any target needed applies to all sides (written to --out, read by
run_all.sh), so no side is measured colder than another.

  warmcheck.py [--targets …] [--endpoints list,hydrate:100] [--steps 3,10,30,60]
               [--window 10] [--conc 16] [--tolerance 0.03] [--out warmup.txt] [--json FILE]
"""

import argparse, json, os, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402
import targets  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(t for _, ts in targets.FRAMEWORKS.values() for t, *_ in ts))
    ap.add_argument("--endpoints", default="list,hydrate:100")
    ap.add_argument("--steps", default="3,10,30,60")
    ap.add_argument("--window", type=int, default=10)
    ap.add_argument("--conc", type=int, default=16)
    ap.add_argument("--tolerance", type=float, default=0.03)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--notes", type=int, default=1000)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/perf/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--out", help="write the warm-up that applies to all sides here (seconds)")
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    a.app_cores, a.db_cores, a.load_cores = bm2.APP_CORES, bm2.DB_CORES, bm2.LOAD_CORES
    a.opengrid, a.opengrid_pgo = os.path.abspath(a.opengrid), os.path.abspath(a.opengrid_pgo)
    steps = [int(s) for s in a.steps.split(",")]
    res = {}
    for name in a.targets.split(","):
        for e in a.endpoints.split(","):
            tries = []
            for w in steps:
                with tempfile.TemporaryDirectory(prefix=f"warm-{name}-") as work:
                    t = bm2.Target(name, a, work)
                    try:
                        t.launch()
                        t.seed()
                        url, method, body, token = t.endpoint(e)
                        bm2.oha(url, a.conc, w, a.load_cores, method, body, token)
                        rps = [bm2.oha(url, a.conc, a.window, a.load_cores, method, body, token)["summary"]["requestsPerSec"] for _ in range(3)]
                    finally:
                        t.close()
                spread = max(rps) / min(rps) - 1 if min(rps) > 0 else float("inf")
                tries.append({"warmup": w, "rps": rps, "spread": spread})
                print(f"  {name:<28} {e:<12} warm-up {w:>2} s  windows {' / '.join(f'{r:.0f}' for r in rps)}  spread {spread * 100:.1f} %", file=sys.stderr)
                if spread <= a.tolerance:
                    break
            ok = tries[-1]["spread"] <= a.tolerance
            res[f"{e}|{name}"] = {"tries": tries, "needed": tries[-1]["warmup"] if ok else None}
    needed = [v["needed"] for v in res.values()]
    # a target stable at no step: the longest step, and it is reported
    applied = max(steps[-1] if n is None else n for n in needed) if needed else steps[0]
    print(f"Warm-up check (BENCH3) · per target and endpoint a fresh server, warm-up, then 3 × {a.window} s at {a.conc} connections; "
          f"sufficient when the windows differ by at most {a.tolerance * 100:.0f} % · steps {a.steps} s · "
          f"**applied to every side: {applied} s**\n")
    print("| Target | Endpoint | Warm-up s | Windows requests/s | Difference | Sufficient from |")
    print("|---|---|---:|---|---:|---|")
    for k, v in res.items():
        e, name = k.split("|")
        for x in v["tries"]:
            print(f"| {name} | {e} | {x['warmup']} | {' / '.join(f'{r:.0f}' for r in x['rps'])} | {x['spread'] * 100:.1f} % | {'' if x is not v['tries'][-1] else (str(v['needed']) + ' s' if v['needed'] is not None else 'no step (not stable)')} |")
    print()
    if a.out:
        open(a.out, "w").write(f"{applied}\n")
    if a.json:
        json.dump({"applied": applied, "points": res}, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
