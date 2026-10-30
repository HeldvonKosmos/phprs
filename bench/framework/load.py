#!/usr/bin/env python3
"""Concurrency and open-loop load (BENCH1): how the targets of bm2.py behave
with many connections and under a fixed request rate, set up exactly as by
bm2.py (same cores, workers, data, durability).

Per target and endpoint:
1. closed loop at --conc connections (default 16, 256, 1024): requests/s,
   p50, p99, p99.9, failures (non-2xx and connection errors);
2. open loop: a fixed rate of 25/50/75/90 % of the closed-loop rate at
   --capacity-conc connections (the target's capacity), sent with
   oha -q and --latency-correction (latency counted from the planned send
   time, so a stalled server is not hidden by coordinated omission), over
   --open-conc connections (default the same 256: the target reached the
   full rate with them, so the client never holds the rate back), each
   level after a warm-up at the same rate;
   the knee is the first level whose p99 is more than twice the p99 at
   25 %;
3. spike: the 25 % stream for --spike-total seconds, and from second
   --spike-at a second stream of 65 % for --spike-len seconds (together
   90 %); reported: p50/p99/p99.9 of the 25 % stream during that run
   against the plain 25 % run, and failures of both streams.

  load.py [--targets …] [--endpoints show,list] [--conc 16,256,1024]
          [--levels 25,50,75,90] [--duration 10] [--warmup 3] [--json FILE]
"""

import argparse, json, os, subprocess, sys, tempfile, threading, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bm2  # noqa: E402

TARGETS = ["opengrid-embedded-pgo-core", "laravel-frankenphp-core"]  # D25: core only


def busy(cores):
    """Busy and total ticks of the given cores (/proc/stat)."""
    want, b, t = set(bm2.cpus(cores)), 0, 0
    for line in open("/proc/stat"):
        f = line.split()
        if f[0].startswith("cpu") and f[0][3:].isdigit() and int(f[0][3:]) in want:
            v = list(map(int, f[1:]))
            b, t = b + sum(v) - v[3] - v[4], t + sum(v)
    return b, t


def run_oha(url, a, method, body, token, duration, conc, rate=None):
    # -w: requests in flight at the deadline finish and count (otherwise oha
    # aborts them and reports one error per connection)
    cmd = ["taskset", "-c", a.load_cores, "oha", "--no-tui", "--output-format", "json", "-w", "-z", f"{duration}s", "-c", str(conc), "-m", method]
    if rate:
        cmd += ["-q", str(rate), "--latency-correction"]
    if token:
        cmd += ["-H", f"Authorization: Bearer {token}"]
    if body is not None:
        cmd += ["-T", "application/json", "-d", json.dumps(body)]
    b0, t0 = busy(a.load_cores)
    out = subprocess.run(cmd + [url], capture_output=True, text=True)
    b1, t1 = busy(a.load_cores)
    if out.returncode != 0:
        sys.exit(f"oha failed: {out.stderr[-500:]}")
    r = json.loads(out.stdout)
    codes = r.get("statusCodeDistribution", {})
    errors = sum(r.get("errorDistribution", {}).values())
    lat = r["latencyPercentiles"]
    ms = lambda k: (lat.get(k) or 0) * 1000
    return {"rps": r["summary"]["requestsPerSec"], "p50": ms("p50"), "p99": ms("p99"), "p999": ms("p99.9"),
            "bad": sum(n for c, n in codes.items() if not c.startswith("2")) + errors, "total": sum(codes.values()) + errors,
            # how busy the load generator's cores were: above 90 % the client may be the limit
            "load_busy": 100 * (b1 - b0) / max(1, t1 - t0)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--targets", default=",".join(TARGETS))
    ap.add_argument("--endpoints", default="show,list")
    ap.add_argument("--conc", default="16,256,1024")
    ap.add_argument("--capacity-conc", type=int, default=256)
    ap.add_argument("--open-conc", type=int, default=256)
    ap.add_argument("--levels", default="25,50,75,90")
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--warmup", type=int, default=3)
    ap.add_argument("--spike-total", type=int, default=16)
    ap.add_argument("--spike-at", type=int, default=5)
    ap.add_argument("--spike-len", type=int, default=4)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--notes", type=int, default=200)
    ap.add_argument("--opengrid", default=os.path.join(bm2.WS, "target/perf/opengrid"))
    ap.add_argument("--opengrid-pgo", default=os.path.join(bm2.WS, "target-pgo/opengrid-pgo"))
    ap.add_argument("--json", help="write the raw results here")
    a = ap.parse_args()
    a.app_cores, a.db_cores, a.load_cores = bm2.APP_CORES, bm2.DB_CORES, bm2.LOAD_CORES
    a.opengrid, a.opengrid_pgo = os.path.abspath(a.opengrid), os.path.abspath(a.opengrid_pgo)
    conc = [int(c) for c in a.conc.split(",")]
    levels = [int(x) for x in a.levels.split(",")]
    res = {}
    print(f"Concurrency and open-loop load (BENCH1) · {a.workers} workers per side · cores: app {a.app_cores} ({bm2.topology(a.app_cores)}), "
          f"database {a.db_cores}, load {a.load_cores} · per point {a.warmup} s warm-up + {a.duration} s · open loop with oha -q --latency-correction "
          f"over {a.open_conc} connections, levels in % of the rate at {a.capacity_conc} connections\n")
    for name in a.targets.split(","):
        with tempfile.TemporaryDirectory(prefix=f"load-{name}-") as work:
            t = bm2.Target(name, a, work)
            try:
                t.launch()
                t.seed()
                for e in a.endpoints.split(","):
                    url, method, body, token = t.endpoint(e)
                    r = res.setdefault(f"{e}|{name}", {"closed": {}, "open": {}})
                    for c in conc:
                        run_oha(url, a, method, body, token, a.warmup, c)
                        r["closed"][c] = run_oha(url, a, method, body, token, a.duration, c)
                        x = r["closed"][c]
                        print(f"  {name:<26} {e:<6} c={c:<5} {x['rps']:>8.0f} req/s p99 {x['p99']:8.2f} ms p99.9 {x['p999']:8.2f} ms failed {x['bad']}", file=sys.stderr)
                    cap = r["closed"].get(a.capacity_conc) or run_oha(url, a, method, body, token, a.duration, a.capacity_conc)
                    r["capacity"] = cap["rps"]
                    for lv in levels:
                        rate = max(1, round(cap["rps"] * lv / 100))
                        # warm-up at the same rate: the connections are open before the measured run
                        run_oha(url, a, method, body, token, a.warmup, a.open_conc, rate)
                        r["open"][lv] = dict(run_oha(url, a, method, body, token, a.duration, a.open_conc, rate), rate=rate)
                        x = r["open"][lv]
                        print(f"  {name:<26} {e:<6} {lv:>3} % = {rate:>6} req/s: p50 {x['p50']:7.2f} p99 {x['p99']:8.2f} p99.9 {x['p999']:8.2f} ms failed {x['bad']}", file=sys.stderr)
                    base_lv = levels[0]
                    base_rate, spike_rate = max(1, round(cap["rps"] * base_lv / 100)), max(1, round(cap["rps"] * 0.65))
                    out = {}
                    run_oha(url, a, method, body, token, a.warmup, a.open_conc, base_rate)
                    def spike():
                        time.sleep(a.spike_at)
                        out["spike"] = run_oha(url, a, method, body, token, a.spike_len, a.open_conc, spike_rate)
                    th = threading.Thread(target=spike)
                    th.start()
                    out["base"] = run_oha(url, a, method, body, token, a.spike_total, a.open_conc, base_rate)
                    th.join()
                    r["spike"] = {"base": out["base"], "spike": out["spike"], "base_rate": base_rate, "spike_rate": spike_rate}
                    print(f"  {name:<26} {e:<6} spike: base p99 {out['base']['p99']:.2f} ms p99.9 {out['base']['p999']:.2f} ms failed {out['base']['bad'] + out['spike']['bad']}", file=sys.stderr)
            finally:
                t.close()
    for e in a.endpoints.split(","):
        print(f"#### {e}: geschlossene Last\n")
        print("| Target | Connections | Requests/s | p50 ms | p99 ms | p99.9 ms | Failed | Load cores busy |")
        print("|---|---:|---:|---:|---:|---:|---:|---:|")
        for name in a.targets.split(","):
            for c in conc:
                x = res[f"{e}|{name}"]["closed"][c]
                flag = " (Lastgenerator am Limit)" if x["load_busy"] > 90 else ""
                print(f"| {name} | {c} | {x['rps']:.0f} | {x['p50']:.2f} | {x['p99']:.2f} | {x['p999']:.2f} | {x['bad']} von {x['total']} | {x['load_busy']:.0f} %{flag} |")
        print(f"\n#### {e}: offene Last (feste Rate) und Knie\n")
        print("| Target | Capacity req/s | " + " | ".join(f"{lv} %: p50 / p99 / p99.9 ms" for lv in levels) + " | Knee | Spike: p99 / p99.9 ms (without → with) | Failed |")
        print("|---|---:|" + "---|" * len(levels) + "---|---|---:|")
        for name in a.targets.split(","):
            r = res[f"{e}|{name}"]
            base = r["open"][levels[0]]["p99"]
            knee = next((f"at {lv} %" for lv in levels[1:] if r["open"][lv]["p99"] > 2 * base), f"none up to {levels[-1]} %")
            cells = [f"{r['open'][lv]['p50']:.2f} / {r['open'][lv]['p99']:.2f} / {r['open'][lv]['p999']:.2f}" for lv in levels]
            s = r["spike"]
            bad = sum(r["open"][lv]["bad"] for lv in levels) + s["base"]["bad"] + s["spike"]["bad"]
            print(f"| {name} | {r['capacity']:.0f} | " + " | ".join(cells) + f" | {knee} | {base:.2f} / {r['open'][levels[0]]['p999']:.2f} → {s['base']['p99']:.2f} / {s['base']['p999']:.2f} | {bad} |")
        print()
    print(f"Knee: the first level whose p99 is more than twice the p99 at {levels[0]} %. Spike: {levels[0]} % base load for {a.spike_total} s, from second "
          f"{a.spike_at} an additional 65 % for {a.spike_len} s (90 % together); shown: p99/p99.9 of the base load without and with the spike.\n")
    if a.json:
        json.dump(res, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
