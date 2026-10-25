#!/bin/bash
# Generate the dialect version of every c*.php: `<?php` becomes `<?grd`, and a
# line containing `// grd: CODE` is replaced by CODE (same indentation) —
# used where the dialect differs, e.g. sort functions that return the array.
cd "$(dirname "$0")"
for php in c*.php; do
  python3 - "$php" "${php%.php}.grd" <<'PY'
import re, sys
src, dst = sys.argv[1], sys.argv[2]
out = []
for i, line in enumerate(open(src).read().split("\n")):
    if i == 0 and line.startswith("<?php"):
        line = "<?grd" + line[5:]
    m = None if line.lstrip().startswith("//") else re.match(r"^(\s*).*//\s*grd:\s*(.+)$", line)
    if m:
        line = m.group(1) + m.group(2)
    out.append(line)
text = "\n".join(out)
text = text.replace("<?grd\n", "<?grd\n// GENERATED from " + src + " by gen.sh — edit the .php file.\n", 1)
open(dst, "w").write(text)
PY
done
