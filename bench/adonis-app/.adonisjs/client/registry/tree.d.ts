/* eslint-disable prettier/prettier */
import type { routes } from './index.ts'

export interface ApiDefinition {
  benchSql: {
    health: typeof routes['bench_sql.health']
    sqlite: typeof routes['bench_sql.sqlite']
    login: typeof routes['bench_sql.login']
    index: typeof routes['bench_sql.index']
    store: typeof routes['bench_sql.store']
    show: typeof routes['bench_sql.show']
  }
  benchLogic: {
    hydrate: typeof routes['bench_logic.hydrate']
    collect: typeof routes['bench_logic.collect']
    validate: typeof routes['bench_logic.validate']
    json: typeof routes['bench_logic.json']
  }
}
