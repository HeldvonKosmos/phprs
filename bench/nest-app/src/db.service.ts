import { Injectable } from '@nestjs/common'
import Database from 'better-sqlite3'

/**
 * Measurement tool (BENCH3): the benchmark's database, raw SQL only (Nest
 * has no database layer of its own). One connection per worker process,
 * WAL, synchronous FULL, busy_timeout 5000 as on every side.
 */
@Injectable()
export class DbService {
  readonly db: Database.Database

  constructor() {
    this.db = new Database(process.env.BENCH_SQLITE_DATABASE ?? 'bench.sqlite')
    this.db.pragma('journal_mode = WAL')
    this.db.pragma('synchronous = FULL')
    this.db.pragma('busy_timeout = 5000')
  }

  select(sql: string, params: unknown[] = []): any[] {
    return this.db.prepare(sql).all(...params)
  }

  run(sql: string, params: unknown[] = []): void {
    this.db.prepare(sql).run(...params)
  }
}
