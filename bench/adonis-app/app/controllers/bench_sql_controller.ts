import { createHash, randomBytes } from 'node:crypto'
import type { HttpContext } from '@adonisjs/core/http'
import { rawQuery } from '#bench/db'

/**
 * Measurement tool (BENCH3): the framework-core benchmark with raw SQL on
 * SQLite. The Laravel app and OpenGrid have the same controller with the
 * same statements and the same answers.
 */
export default class BenchSqlController {
  health({ response }: HttpContext) {
    return response.json({ status: 'ok' })
  }

  /** The SQLite this side runs and the runtime, for the header. */
  async sqlite({ response }: HttpContext) {
    const version = rawQuery('select sqlite_version() AS v')[0].v
    const options = rawQuery('pragma compile_options').map((r) => r.compile_options)
    return response.json({ version, compile_options: options, runtime: { bun: Bun.version, workers: Number(process.env.WORKERS ?? 4) } })
  }

  async login({ request, response }: HttpContext) {
    const email = request.input('email')
    const password = request.input('password')
    if (typeof email !== 'string' || typeof password !== 'string') {
      return response.status(422).json({ message: 'The email and password fields are required.' })
    }
    const rows = rawQuery('SELECT id, password FROM users WHERE email = ? LIMIT 1', [email])
    if (rows.length === 0 || !(await Bun.password.verify(password, rows[0].password, 'bcrypt'))) {
      return response.status(401).json({ message: 'Invalid credentials.' })
    }
    const token = randomBytes(32).toString('hex')
    const now = new Date().toISOString().slice(0, 19).replace('T', ' ')
    rawQuery('INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)', [rows[0].id, createHash('sha256').update(token).digest('hex'), Math.floor(Date.now() / 1000), now])
    return response.json({ token })
  }

  /** GET /api/bench/sql/notes?done=false&per_page=15&page=1 */
  async index(ctx: HttpContext) {
    const user = (ctx as any).benchUser
    const qs = ctx.request.qs()
    const done = (qs.done ?? 'false') === 'true' ? 1 : 0
    const perPage = Number.parseInt(qs.per_page ?? '15', 10) || 0
    const page = Number.parseInt(qs.page ?? '1', 10) || 0
    const total = rawQuery('SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL', [user.id, done])[0].total
    const rows = rawQuery('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?', [user.id, done, perPage, (page - 1) * perPage])
    return ctx.response.json({ data: rows.map(note), total, per_page: perPage, current_page: page })
  }

  async show(ctx: HttpContext) {
    const user = (ctx as any).benchUser
    const id = Number.parseInt(ctx.params.id, 10) || 0
    const rows = rawQuery('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1', [id, user.id])
    if (rows.length === 0) {
      return ctx.response.status(404).json({ message: 'Not found.' })
    }
    return ctx.response.json({ data: note(rows[0]) })
  }

  async store(ctx: HttpContext) {
    const user = (ctx as any).benchUser
    const title = ctx.request.input('title')
    if (typeof title !== 'string' || title === '') {
      return ctx.response.status(422).json({ message: 'The title field is required.' })
    }
    const rawBody = ctx.request.input('body')
    const body = typeof rawBody === 'string' ? rawBody : null
    const rawPriority = ctx.request.input('priority', 3)
    const priority = Number.isInteger(rawPriority) ? rawPriority : 3
    const now = new Date().toISOString().slice(0, 19).replace('T', ' ')
    const id = rawQuery('INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id', [user.id, title, body, priority, now, now])[0].id
    return ctx.response.status(201).json({ data: { id, title, body, done: false, priority, created_at: now, updated_at: now } })
  }
}

function note(row: any) {
  return { id: row.id, title: row.title, body: row.body, done: row.done === 1, priority: row.priority, created_at: row.created_at, updated_at: row.updated_at }
}
