import { createHash } from 'node:crypto'
import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'
import { rawQuery } from '#bench/db'

/**
 * Measurement tool (BENCH3): the token check of the framework-core
 * benchmark, the same statements as in the Laravel app. The token's use is
 * written at most once a second, as Sanctum does in effect.
 */
export default class BenchTokenMiddleware {
  async handle(ctx: HttpContext, next: NextFn) {
    const header = ctx.request.header('authorization') ?? ''
    const token = header.startsWith('Bearer ') ? header.slice(7) : ''
    if (token === '') {
      return ctx.response.status(401).json({ message: 'Unauthenticated.' })
    }
    const rows = rawQuery('SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1', [createHash('sha256').update(token).digest('hex')])
    if (rows.length === 0) {
      return ctx.response.status(401).json({ message: 'Unauthenticated.' })
    }
    const row = rows[0]
    const now = Math.floor(Date.now() / 1000)
    if (now - row.last_used_at >= 1) {
      rawQuery('UPDATE tokens SET last_used_at = ? WHERE id = ?', [now, row.token_id])
    }
    ;(ctx as any).benchUser = { id: row.id, name: row.name, email: row.email }
    return next()
  }
}
