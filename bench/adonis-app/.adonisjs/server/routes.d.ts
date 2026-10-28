import '@adonisjs/core/types/http'

type ParamValue = string | number | bigint | boolean

export type ScannedRoutes = {
  ALL: {
    'bench_sql.health': { paramsTuple?: []; params?: {} }
    'bench_sql.sqlite': { paramsTuple?: []; params?: {} }
    'bench_sql.login': { paramsTuple?: []; params?: {} }
    'bench_sql.index': { paramsTuple?: []; params?: {} }
    'bench_sql.store': { paramsTuple?: []; params?: {} }
    'bench_sql.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'bench_logic.hydrate': { paramsTuple?: []; params?: {} }
    'bench_logic.collect': { paramsTuple?: []; params?: {} }
    'bench_logic.validate': { paramsTuple?: []; params?: {} }
    'bench_logic.json': { paramsTuple?: []; params?: {} }
  }
  GET: {
    'bench_sql.health': { paramsTuple?: []; params?: {} }
    'bench_sql.sqlite': { paramsTuple?: []; params?: {} }
    'bench_sql.index': { paramsTuple?: []; params?: {} }
    'bench_sql.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'bench_logic.hydrate': { paramsTuple?: []; params?: {} }
    'bench_logic.collect': { paramsTuple?: []; params?: {} }
    'bench_logic.json': { paramsTuple?: []; params?: {} }
  }
  HEAD: {
    'bench_sql.health': { paramsTuple?: []; params?: {} }
    'bench_sql.sqlite': { paramsTuple?: []; params?: {} }
    'bench_sql.index': { paramsTuple?: []; params?: {} }
    'bench_sql.show': { paramsTuple: [ParamValue]; params: {'id': ParamValue} }
    'bench_logic.hydrate': { paramsTuple?: []; params?: {} }
    'bench_logic.collect': { paramsTuple?: []; params?: {} }
    'bench_logic.json': { paramsTuple?: []; params?: {} }
  }
  POST: {
    'bench_sql.login': { paramsTuple?: []; params?: {} }
    'bench_sql.store': { paramsTuple?: []; params?: {} }
    'bench_logic.validate': { paramsTuple?: []; params?: {} }
  }
}
declare module '@adonisjs/core/types/http' {
  export interface RoutesList extends ScannedRoutes {}
}