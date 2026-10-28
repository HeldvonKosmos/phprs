/* eslint-disable prettier/prettier */
import type { AdonisEndpoint } from '@tuyau/core/types'
import type { Registry } from './schema.d.ts'
import type { ApiDefinition } from './tree.d.ts'

export type { Route, Path } from './schema.d.ts'

const placeholder: any = {}

const routes = {
  'bench_sql.health': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/sql/health',
    tokens: [[0,"api"],[0,"bench"],[0,"sql"],[0,"health"]],
    types: placeholder as Registry['bench_sql.health']['types'],
  },
  'bench_sql.sqlite': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/sql/sqlite',
    tokens: [[0,"api"],[0,"bench"],[0,"sql"],[0,"sqlite"]],
    types: placeholder as Registry['bench_sql.sqlite']['types'],
  },
  'bench_sql.login': {
    methods: ["POST"],
    pattern: '/api/bench/sql/login',
    tokens: [[0,"api"],[0,"bench"],[0,"sql"],[0,"login"]],
    types: placeholder as Registry['bench_sql.login']['types'],
  },
  'bench_sql.index': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/sql/notes',
    tokens: [[0,"api"],[0,"bench"],[0,"sql"],[0,"notes"]],
    types: placeholder as Registry['bench_sql.index']['types'],
  },
  'bench_sql.store': {
    methods: ["POST"],
    pattern: '/api/bench/sql/notes',
    tokens: [[0,"api"],[0,"bench"],[0,"sql"],[0,"notes"]],
    types: placeholder as Registry['bench_sql.store']['types'],
  },
  'bench_sql.show': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/sql/notes/:id',
    tokens: [[0,"api"],[0,"bench"],[0,"sql"],[0,"notes"],[1,"id"]],
    types: placeholder as Registry['bench_sql.show']['types'],
  },
  'bench_logic.hydrate': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/logic/hydrate',
    tokens: [[0,"api"],[0,"bench"],[0,"logic"],[0,"hydrate"]],
    types: placeholder as Registry['bench_logic.hydrate']['types'],
  },
  'bench_logic.collect': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/logic/collect',
    tokens: [[0,"api"],[0,"bench"],[0,"logic"],[0,"collect"]],
    types: placeholder as Registry['bench_logic.collect']['types'],
  },
  'bench_logic.validate': {
    methods: ["POST"],
    pattern: '/api/bench/logic/validate',
    tokens: [[0,"api"],[0,"bench"],[0,"logic"],[0,"validate"]],
    types: placeholder as Registry['bench_logic.validate']['types'],
  },
  'bench_logic.json': {
    methods: ["GET","HEAD"],
    pattern: '/api/bench/logic/json',
    tokens: [[0,"api"],[0,"bench"],[0,"logic"],[0,"json"]],
    types: placeholder as Registry['bench_logic.json']['types'],
  },
} as const satisfies Record<string, AdonisEndpoint>

export { routes }

export const registry = {
  routes,
  $tree: {} as ApiDefinition,
}

declare module '@tuyau/core/types' {
  export interface UserRegistry {
    routes: typeof routes
    $tree: ApiDefinition
  }
}
