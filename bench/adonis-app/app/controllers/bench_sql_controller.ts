import { createHash, randomBytes } from 'node:crypto'
import type { HttpContext } from '@adonisjs/core/http'
import db from '@adonisjs/lucid/services/db'
import bcrypt from 'bcrypt'

/**
 * Measurement tool (BENCH3): the framework-core benchmark with raw SQL on
 * SQLite. The Laravel app and OpenGrid have the same controller with the
 * same statements and the same answers.
 */
export default class BenchSqlController {
  health({ response }: HttpContext) {
    return response.json({ status: 'ok' })
  }

  async login({ request, response }: HttpContext) {
    const email = request.input('email')
    const password = request.input('password')
    if (typeof email !== 'string' || typeof password !== 'string') {
      return response.status(422).json({ message: 'The email and password fields are required.' })
    }
    const conn = db.connection('bench')
    const rows = (await conn.rawQuery('SELECT id, password FROM users WHERE email = ? LIMIT 1', [email])) as any[]
    if (rows.length === 0 || !(await bcrypt.compare(password, rows[0].password))) {
      return response.status(401).json({ message: 'Invalid credentials.' })
    }
    const token = randomBytes(32).toString('hex')
    const now = new Date().toISOString().slice(0, 19).replace('T', ' ')
    await conn.rawQuery('INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)', [rows[0].id, createHash('sha256').update(token).digest('hex'), Math.floor(Date.now() / 1000), now])
    return response.json({ token })
  }

  /** GET /api/bench/sql/notes?done=false&per_page=15&page=1 */
  async index(ctx: HttpContext) {
    const user = (ctx as any).benchUser
    const qs = ctx.request.qs()
    const done = (qs.done ?? 'false') === 'true' ? 1 : 0
    const perPage = Number.parseInt(qs.per_page ?? '15', 10) || 0
    const page = Number.parseInt(qs.page ?? '1', 10) || 0
    const conn = db.connection('bench')
    const total = ((await conn.rawQuery('SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL', [user.id, done])) as any[])[0].total
    const rows = (await conn.rawQuery('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?', [user.id, done, perPage, (page - 1) * perPage])) as any[]
    return ctx.response.json({ data: rows.map(note), total, per_page: perPage, current_page: page })
  }

  async show(ctx: HttpContext) {
    const user = (ctx as any).benchUser
    const id = Number.parseInt(ctx.params.id, 10) || 0
    const rows = (await db.connection('bench').rawQuery('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1', [id, user.id])) as any[]
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
    const id = ((await db.connection('bench').rawQuery('INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id', [user.id, title, body, priority, now, now])) as any[])[0].id
    return ctx.response.status(201).json({ data: { id, title, body, done: false, priority, created_at: now, updated_at: now } })
  }
}

function note(row: any) {
  return { id: row.id, title: row.title, body: row.body, done: row.done === 1, priority: row.priority, created_at: row.created_at, updated_at: row.updated_at }
}
