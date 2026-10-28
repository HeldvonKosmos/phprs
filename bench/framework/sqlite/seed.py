#!/usr/bin/env python3
"""A fresh SQLite database for the framework-core benchmark (D23, DBR1):
schema.sql, one user (bench@example.com / secret-123, bcrypt cost 10 from
PHP's password_hash with the prefix $2b$, verified with every side's bcrypt) and the
notes of bm2.py (title "Note NNNN", body of 40 x, done every second,
priority 1-5). Uses the system SQLite through Python's sqlite3, in WAL mode.

  seed.py <database file> [notes]
"""

import os, sqlite3, subprocess, sys

path = sys.argv[1]
notes = int(sys.argv[2]) if len(sys.argv) > 2 else 200
for suffix in ("", "-wal", "-shm"):
    if os.path.exists(path + suffix):
        os.remove(path + suffix)
here = os.path.dirname(os.path.abspath(__file__))
pw = subprocess.run(["php", "-r", "echo password_hash('secret-123', PASSWORD_BCRYPT, ['cost' => 10]);"], capture_output=True, text=True, check=True).stdout
# BENCH3: the same hash with the prefix $2b$ (PHP writes $2y$, the same
# algorithm); $2b$ is what every bcrypt library accepts (Node's bcrypt
# rejects $2y$), PHP and OpenGrid verify it too
assert pw.startswith("$2y$10$"), pw
pw = "$2b$" + pw[4:]
db = sqlite3.connect(path)
db.execute("PRAGMA journal_mode = WAL")
db.executescript(open(os.path.join(here, "schema.sql")).read())
now = "2026-10-27 00:00:00"
db.execute("INSERT INTO users (id, name, email, password, created_at) VALUES (1, 'Bench', 'bench@example.com', ?, ?)", (pw, now))
db.executemany(
    "INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (1, ?, ?, ?, ?, ?, ?)",
    [(f"Note {i:04d}", "x" * 40, 1 if i % 2 == 0 else 0, i % 5 + 1, now, now) for i in range(notes)],
)
db.commit()
db.close()
print(f"seeded {path}: 1 user, {notes} notes")
