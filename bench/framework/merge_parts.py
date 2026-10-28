#!/usr/bin/env python3
"""Joins the per-framework parts of a stage (BENCH3): run_all.sh runs every
stage once per framework into <run>/parts/<stage>-<fw>.{json,md}; this
writes <run>/<stage>.json (the parts' keys all carry the target, so they
are joined as they are) and <run>/<stage>.md (the parts one after another),
the files report.py and series.py read.

  merge_parts.py <run dir> <stage> ...
"""

import glob, json, os, sys


def main():
    run = sys.argv[1]
    for stage in sys.argv[2:]:
        out, md = {}, []
        for p in sorted(glob.glob(os.path.join(run, "parts", f"{stage}-*.json"))):
            out.update(json.load(open(p)))
            m = p[:-5] + ".md"
            if os.path.exists(m):
                md.append(f"<!-- {os.path.basename(m)} -->\n" + open(m).read().rstrip("\n") + "\n")
        if out:
            json.dump(out, open(os.path.join(run, f"{stage}.json"), "w"), indent=1)
            open(os.path.join(run, f"{stage}.md"), "w").write("\n".join(md))


if __name__ == "__main__":
    main()
