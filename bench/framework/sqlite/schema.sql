-- Schema of the framework-core benchmark (D23, DBR1): both apps run the
-- same raw SQL against this SQLite database. A measurement tool, not an
-- application schema.
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE tokens (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    last_used_at INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE notes (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    body TEXT,
    done INTEGER NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 3,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);
CREATE INDEX notes_user_done_deleted_priority ON notes (user_id, done, deleted_at, priority);
CREATE INDEX notes_user_deleted_priority ON notes (user_id, deleted_at, priority);
