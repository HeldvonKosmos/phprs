#!/usr/bin/env python3
"""The runs of bench/run_all.sh --runs N as one result: the median of every
measured value over the runs and, for the request rates, their range.

  bench/series.py <result dir> <run dir> ...   (run_all.sh: <result dir>/run-<i>)

Writes into <series dir> the same files a single run has, so report.py reads
it unchanged, with medians:
- bm2.json, thesis.json: per point the median of requests/s, p50, p99 and
  requests in flight; non-2xx responses and requests summed over the runs;
  bm2.range.json, thesis.range.json: per point [min, max] of requests/s;
- load.json: median of every number (failures and requests summed);
  load.range.json: [min, max] of the closed-loop requests/s and capacity;
- memory.json: median of every number;
- warmcurve.json: per bin the median of requests/s, p50, p99 over the
  runs, and the median of the steady rate and the times to warm;
- coldstart.json: the start, first-request and warm times of all runs
  pooled (the report takes their median); hot reload likewise;
- env.txt of the first run plus the list of runs, stages.txt and gates.txt
  of every run, quiet.txt of every run, lang.md of every run (each already
  a median of 7), the runs' report.md not copied.
"""

import json, os, shutil, statistics, sys

SUM = {"bad", "total", "failed"}


def med(values):
    return statistics.median(values)


def merge_points(runs):
    """bm2/thesis: key -> [rps, p50, p99, bad, total, inflight]."""
    out, rng = {}, {}
    for k in runs[0]:
        vals = [r[k] for r in runs if k in r]
        if k.startswith(("answer|", "runtime|")):
            out[k] = vals[0]
            continue
        out[k] = [med([v[0] for v in vals]), med([v[1] for v in vals]), med([v[2] for v in vals]),
                  sum(v[3] for v in vals), sum(v[4] for v in vals), med([v[5] for v in vals])]
        rng[k] = [min(v[0] for v in vals), max(v[0] for v in vals)]
    return out, rng


def merge_tree(nodes, key=None):
    """Median of every number of equally shaped JSON trees (sums for failure counts)."""
    first = nodes[0]
    if isinstance(first, dict):
        return {k: merge_tree([n[k] for n in nodes if k in n], k) for k in first}
    if isinstance(first, list) and first and all(isinstance(x, (int, float)) for x in first) and key in ("idle", "after"):
        return [med([n[i] for n in nodes]) for i in range(len(first))]
    if isinstance(first, list):
        return [x for n in nodes for x in n]  # samples: pooled
    if isinstance(first, bool) or not isinstance(first, (int, float)):
        return first
    return sum(nodes) if key in SUM else med(nodes)


def load_range(runs):
    rng = {}
    for k in runs[0]:
        vals = [r[k] for r in runs if k in r]
        rng[k] = {"closed": {c: [min(v["closed"][c]["rps"] for v in vals), max(v["closed"][c]["rps"] for v in vals)] for c in vals[0]["closed"]},
                  "capacity": [min(v["capacity"] for v in vals), max(v["capacity"] for v in vals)]}
    return rng


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    out, dirs = sys.argv[1], sys.argv[2:]
    os.makedirs(out, exist_ok=True)
    js = lambda d, n: json.load(open(os.path.join(d, n))) if os.path.exists(os.path.join(d, n)) else None
    for name in ("bm2.json", "thesis.json"):
        runs = [x for x in (js(d, name) for d in dirs) if x is not None]
        if runs:
            m, r = merge_points(runs)
            json.dump(m, open(os.path.join(out, name), "w"), indent=1)
            json.dump(r, open(os.path.join(out, name.replace(".json", ".range.json")), "w"), indent=1)
    runs = [x for x in (js(d, "load.json") for d in dirs) if x is not None]
    if runs:
        json.dump(merge_tree(runs), open(os.path.join(out, "load.json"), "w"), indent=1)
        json.dump(load_range(runs), open(os.path.join(out, "load.range.json"), "w"), indent=1)
    # warm-up curves: per bin the median over the runs; the derived values too
    runs = [x for x in (js(d, "warmcurve.json") for d in dirs) if x is not None]
    if runs:
        out_c = {}
        for k in runs[0]:
            vals = [r[k] for r in runs if k in r]
            n = min(len(v["points"]) for v in vals)
            pts = [{q: (med([v["points"][i][q] for v in vals if v["points"][i][q] is not None]) if any(v["points"][i][q] is not None for v in vals) else None) for q in ("rps", "p50", "p99")} for i in range(n)]
            nn = lambda q: [v[q] for v in vals if v[q] is not None]
            out_c[k] = {"points": pts, "bad": sum(v["bad"] for v in vals), "steady": med([v["steady"] for v in vals]), "first": med([v["first"] for v in vals]),
                        "warm90": med(nn("warm90")) if nn("warm90") else None, "warm97": med(nn("warm97")) if nn("warm97") else None,
                        "range_steady": [min(v["steady"] for v in vals), max(v["steady"] for v in vals)]}
        json.dump(out_c, open(os.path.join(out, "warmcurve.json"), "w"), indent=1)
    for name in ("memory.json", "coldstart.json", "hotreload.json"):
        runs = [x for x in (js(d, name) for d in dirs) if x is not None]
        if runs:
            json.dump(merge_tree(runs), open(os.path.join(out, name), "w"), indent=1)
    env = open(os.path.join(dirs[0], "env.txt")).read()
    last = open(os.path.join(dirs[-1], "env.txt")).read()
    fin = [l for l in last.splitlines() if l.startswith("finished ")]
    with open(os.path.join(out, "env.txt"), "w") as f:
        f.write(env.rstrip("\n") + "\n")
        if fin:
            f.write(f"series finished {fin[-1].split(' ', 1)[1]}\n")
        f.write(f"series {len(dirs)} runs: " + " ".join(os.path.basename(os.path.normpath(d)) for d in dirs) + "\n")
    for name in ("stages.txt", "gates.txt", "quiet.txt"):
        with open(os.path.join(out, name), "w") as f:
            for i, d in enumerate(dirs, 1):
                p = os.path.join(d, name)
                if os.path.exists(p):
                    f.write(f"# run {i}: {os.path.basename(os.path.normpath(d))}\n" + open(p).read().rstrip("\n") + "\n")
    with open(os.path.join(out, "lang.md"), "w") as f:
        for i, d in enumerate(dirs, 1):
            p = os.path.join(d, "lang.md")
            if os.path.exists(p):
                f.write(f"#### Lauf {i} ({os.path.basename(os.path.normpath(d))}, Median aus 7)\n\n" + open(p).read().rstrip("\n") + "\n\n")
    for name in ("bm2.md", "thesis.md", "load.md", "memory.md", "coldstart.md", "hotreload.md", "warmcurve.md"):
        # the per-run tables stay in the run directories; the series has the report
        if os.path.exists(os.path.join(out, name)):
            os.remove(os.path.join(out, name))
    print(f"series: {len(dirs)} runs merged into {out}")


if __name__ == "__main__":
    main()
