import { createHash, randomBytes } from 'node:crypto'
import { Body, Controller, Get, HttpCode, HttpException, Param, Post, Query, Req, UseGuards } from '@nestjs/common'
import bcrypt from 'bcrypt'
import { BenchTokenGuard } from './bench-token.guard.js'
import { DbService } from './db.service.js'

const now = () => new Date().toISOString().slice(0, 19).replace('T', ' ')
const note = (r: any) => ({ id: r.id, title: r.title, body: r.body, done: r.done === 1, priority: r.priority, created_at: r.created_at, updated_at: r.updated_at })

/**
 * Measurement tool (BENCH3): the framework-core benchmark with raw SQL on
 * SQLite; the same statements and answers as the Laravel app's
 * BenchSqlController.
 */
@Controller('api/bench/sql')
export class BenchSqlController {
  constructor(private readonly db: DbService) {}

  @Get('health')
  health() {
    return { status: 'ok' }
  }

  @Post('login')
  @HttpCode(200)
  async login(@Body() body: any) {
    const { email, password } = body ?? {}
    if (typeof email !== 'string' || typeof password !== 'string') throw new HttpException({ message: 'The email and password fields are required.' }, 422)
    const rows = this.db.select('SELECT id, password FROM users WHERE email = ? LIMIT 1', [email])
    if (rows.length === 0 || !(await bcrypt.compare(password, rows[0].password))) throw new HttpException({ message: 'Invalid credentials.' }, 401)
    const token = randomBytes(32).toString('hex')
    this.db.run('INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)', [rows[0].id, createHash('sha256').update(token).digest('hex'), Math.floor(Date.now() / 1000), now()])
    return { token }
  }

  /** GET /api/bench/sql/notes?done=false&per_page=15&page=1 */
  @Get('notes')
  @UseGuards(BenchTokenGuard)
  index(@Req() req: any, @Query() q: any) {
    const done = (q.done ?? 'false') === 'true' ? 1 : 0
    const perPage = Number.parseInt(q.per_page ?? '15', 10) || 0
    const page = Number.parseInt(q.page ?? '1', 10) || 0
    const total = this.db.select('SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL', [req.benchUser.id, done])[0].total
    const rows = this.db.select('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?', [req.benchUser.id, done, perPage, (page - 1) * perPage])
    return { data: rows.map(note), total, per_page: perPage, current_page: page }
  }

  @Get('notes/:id')
  @UseGuards(BenchTokenGuard)
  show(@Req() req: any, @Param('id') id: string) {
    const rows = this.db.select('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1', [Number.parseInt(id, 10) || 0, req.benchUser.id])
    if (rows.length === 0) throw new HttpException({ message: 'Not found.' }, 404)
    return { data: note(rows[0]) }
  }

  @Post('notes')
  @UseGuards(BenchTokenGuard)
  store(@Req() req: any, @Body() body: any) {
    const title = body?.title
    if (typeof title !== 'string' || title === '') throw new HttpException({ message: 'The title field is required.' }, 422)
    const text = typeof body.body === 'string' ? body.body : null
    const priority = Number.isInteger(body.priority ?? 3) ? (body.priority ?? 3) : 3
    const t = now()
    const id = this.db.select('INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id', [req.benchUser.id, title, text, priority, t, t])[0].id
    return { data: { id, title, body: text, done: false, priority, created_at: t, updated_at: t } }
  }
}
