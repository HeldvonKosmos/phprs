import { createHash } from 'node:crypto'
import { CanActivate, ExecutionContext, HttpException, Injectable } from '@nestjs/common'
import { DbService } from './db.service.js'

/**
 * Measurement tool (BENCH3): the token check of the framework-core
 * benchmark, the same statements as in the Laravel app; the token's use is
 * written at most once a second, as Sanctum does in effect.
 */
@Injectable()
export class BenchTokenGuard implements CanActivate {
  constructor(private readonly db: DbService) {}

  canActivate(context: ExecutionContext): boolean {
    const request = context.switchToHttp().getRequest()
    const header: string = request.headers['authorization'] ?? ''
    const token = header.startsWith('Bearer ') ? header.slice(7) : ''
    if (token === '') throw new HttpException({ message: 'Unauthenticated.' }, 401)
    const rows = this.db.select('SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1', [createHash('sha256').update(token).digest('hex')])
    if (rows.length === 0) throw new HttpException({ message: 'Unauthenticated.' }, 401)
    const row = rows[0]
    const now = Math.floor(Date.now() / 1000)
    if (now - row.last_used_at >= 1) this.db.run('UPDATE tokens SET last_used_at = ? WHERE id = ?', [now, row.token_id])
    request.benchUser = { id: row.id, name: row.name, email: row.email }
    return true
  }
}
