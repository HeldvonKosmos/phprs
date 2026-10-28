/* eslint-disable prettier/prettier */
/// <reference path="../manifest.d.ts" />

import type { ExtractBody, ExtractErrorResponse, ExtractQuery, ExtractQueryForGet, ExtractRawResponse, ExtractResponse, Method, PathWithRegistry, PatternsByMethod, RouteWithRegistry } from '@tuyau/core/types'
import type { InferInput, SimpleError } from '@vinejs/vine/types'
import type { routes } from './index.ts'

export type ParamValue = string | number | bigint | boolean

export interface Registry {
  'bench_sql.health': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/sql/health'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['health']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['health']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['health']>>>
    }
  }
  'bench_sql.sqlite': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/sql/sqlite'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['sqlite']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['sqlite']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['sqlite']>>>
    }
  }
  'bench_sql.login': {
    methods: ["POST"]
    pattern: '/api/bench/sql/login'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['login']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['login']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['login']>>>
    }
  }
  'bench_sql.index': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/sql/notes'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['index']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['index']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['index']>>>
    }
  }
  'bench_sql.store': {
    methods: ["POST"]
    pattern: '/api/bench/sql/notes'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['store']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['store']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['store']>>>
    }
  }
  'bench_sql.show': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/sql/notes/:id'
    types: {
      body: {}
      paramsTuple: [ParamValue]
      params: { id: ParamValue }
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['show']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['show']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_sql_controller').default['show']>>>
    }
  }
  'bench_logic.hydrate': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/logic/hydrate'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['hydrate']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['hydrate']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['hydrate']>>>
    }
  }
  'bench_logic.collect': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/logic/collect'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['collect']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['collect']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['collect']>>>
    }
  }
  'bench_logic.validate': {
    methods: ["POST"]
    pattern: '/api/bench/logic/validate'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['validate']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['validate']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['validate']>>>
    }
  }
  'bench_logic.json': {
    methods: ["GET","HEAD"]
    pattern: '/api/bench/logic/json'
    types: {
      body: {}
      paramsTuple: []
      params: {}
      query: {}
      response: ExtractResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['json']>>>
      errorResponse: ExtractErrorResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['json']>>>
      rawResponse: ExtractRawResponse<Awaited<ReturnType<import('#controllers/bench_logic_controller').default['json']>>>
    }
  }
}

export namespace Route {
  export type Request<Name extends keyof typeof routes> = RouteWithRegistry.Request<typeof routes, Name>
  export type Response<Name extends keyof typeof routes> = RouteWithRegistry.Response<typeof routes, Name>
  export type Params<Name extends keyof typeof routes> = RouteWithRegistry.Params<typeof routes, Name>
  export type Body<Name extends keyof typeof routes> = RouteWithRegistry.Body<typeof routes, Name>
  export type Query<Name extends keyof typeof routes> = RouteWithRegistry.Query<typeof routes, Name>
  export type Error<Name extends keyof typeof routes> = RouteWithRegistry.Error<typeof routes, Name>
}

export namespace Path {
  export type Request<M extends Method, P extends PatternsByMethod<typeof routes, M>> =
    PathWithRegistry.Request<typeof routes, M, P>
  export type Response<M extends Method, P extends PatternsByMethod<typeof routes, M>> =
    PathWithRegistry.Response<typeof routes, M, P>
  export type Params<M extends Method, P extends PatternsByMethod<typeof routes, M>> =
    PathWithRegistry.Params<typeof routes, M, P>
  export type Body<M extends Method, P extends PatternsByMethod<typeof routes, M>> =
    PathWithRegistry.Body<typeof routes, M, P>
  export type Query<M extends Method, P extends PatternsByMethod<typeof routes, M>> =
    PathWithRegistry.Query<typeof routes, M, P>
  export type Error<M extends Method, P extends PatternsByMethod<typeof routes, M>> =
    PathWithRegistry.Error<typeof routes, M, P>
}
