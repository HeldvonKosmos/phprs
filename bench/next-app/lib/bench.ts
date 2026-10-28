import { createHash } from 'node:crypto'
import Database from 'better-sqlite3'

/**
 * Measurement tool (BENCH3): the benchmark's database for the Route
 * Handlers, raw SQL only (Next.js has no database layer). One connection
 * per worker process, WAL, synchronous FULL, busy_timeout 5000 as on every
 * side.
 */
let db: Database.Database | null = null

export function conn(): Database.Database {
  if (db === null) {
    db = new Database(process.env.BENCH_SQLITE_DATABASE ?? 'bench.sqlite')
    db.pragma('journal_mode = WAL')
    db.pragma('synchronous = FULL')
    db.pragma('busy_timeout = 5000')
  }
  return db
}

export const json = (data: unknown, status = 200) => Response.json(data, { status })
export const now = () => new Date().toISOString().slice(0, 19).replace('T', ' ')

export type BenchUser = { id: number; name: string; email: string }

/**
 * The token check of the framework-core benchmark (BenchToken), around a
 * Route Handler; the token's use is written at most once a second.
 */
export function withToken<C>(handler: (req: Request, user: BenchUser, ctx: C) => Response | Promise<Response>) {
  return (req: Request, ctx: C) => {
    const header = req.headers.get('authorization') ?? ''
    const token = header.startsWith('Bearer ') ? header.slice(7) : ''
    if (token === '') return json({ message: 'Unauthenticated.' }, 401)
    const row: any = conn().prepare('SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1').get(createHash('sha256').update(token).digest('hex'))
    if (!row) return json({ message: 'Unauthenticated.' }, 401)
    const t = Math.floor(Date.now() / 1000)
    if (t - row.last_used_at >= 1) conn().prepare('UPDATE tokens SET last_used_at = ? WHERE id = ?').run(t, row.token_id)
    return handler(req, { id: row.id, name: row.name, email: row.email }, ctx)
  }
}

export const note = (r: any) => ({ id: r.id, title: r.title, body: r.body, done: r.done === 1, priority: r.priority, created_at: r.created_at, updated_at: r.updated_at })

export function logicN(req: Request): number {
  const v = Number.parseInt(new URL(req.url).searchParams.get('n') ?? '1', 10)
  return Math.min(1000, Math.max(0, Number.isNaN(v) ? 0 : v))
}

export function logicRows(user: BenchUser, n: number): any[] {
  return conn().prepare('SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?').all(user.id, n)
}

export function sqliteInfo() {
  const c = conn()
  return { version: (c.prepare('select sqlite_version() AS v').get() as any).v, compile_options: c.prepare('pragma compile_options').all().map((r: any) => r.compile_options), runtime: { node: process.versions.node, workers: Number(process.env.WORKERS ?? 4) } }
}
