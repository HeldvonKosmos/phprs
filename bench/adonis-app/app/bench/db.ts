import { Database } from 'bun:sqlite'

/**
 * Measurement tool (BENCH3): the benchmark's database on Bun. Lucid's
 * query client runs on knex with better-sqlite3, which Bun does not load;
 * the raw SQL statements go through bun:sqlite instead (Bun's built-in
 * SQLite 3.53.4, the benchmark's version on every side). One connection
 * per worker process, WAL, synchronous FULL, busy_timeout 5000.
 */
let conn: Database | null = null

export function bench(): Database {
  if (conn === null) {
    conn = new Database(process.env.BENCH_SQLITE_DATABASE ?? 'bench.sqlite')
    conn.run('PRAGMA journal_mode = WAL')
    conn.run('PRAGMA synchronous = FULL')
    conn.run('PRAGMA busy_timeout = 5000')
  }
  return conn
}

/** The rows of a statement (prepared once per process and cached by bun:sqlite). */
export function rawQuery(sql: string, bindings: unknown[] = []): any[] {
  return bench().query(sql).all(...(bindings as any[]))
}
