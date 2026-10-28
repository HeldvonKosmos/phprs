import { Body, Controller, Get, HttpCode, HttpException, Post, Query, Req, UseGuards, ValidationError, ValidationPipe } from '@nestjs/common'
import { Type } from 'class-transformer'
import { ArrayNotEmpty, IsArray, IsInt, IsOptional, IsString, Max, MaxLength, Min, ValidateNested } from 'class-validator'
import { BenchTokenGuard } from './bench-token.guard.js'
import { DbService } from './db.service.js'

/** Measurement tool (BENCH3, THESIS1): the user of a hydrated note. */
class UserView {
  constructor(public id: number, public name: string, public email: string) {}
  toArray() {
    return { id: this.id, name: this.name, email: this.email }
  }
}

/** Measurement tool (BENCH3, THESIS1): a note hydrated from a raw row, with its user. */
class NoteView {
  constructor(public id: number, public title: string, public body: string | null, public done: boolean, public priority: number, public createdAt: string, public user: UserView) {}
  static fromRow(r: any) {
    return new NoteView(r.id, r.title, r.body, r.done === 1, r.priority, r.created_at, new UserView(r.user_id, r.user_name, r.user_email))
  }
  toArray() {
    return { id: this.id, title: this.title, body: this.body, done: this.done, priority: this.priority, created_at: this.createdAt, user: this.user.toArray() }
  }
}

class ItemDto {
  @IsString() @MaxLength(120) title!: string
  @IsInt() @Min(1) @Max(5) priority!: number
  @IsOptional() @IsArray() @IsString({ each: true }) @MaxLength(20, { each: true }) tags?: string[]
}

class ItemsDto {
  // Laravel's `required` fails on an empty list too
  @IsArray() @ArrayNotEmpty() @ValidateNested({ each: true }) @Type(() => ItemDto) items!: ItemDto[]
}

const ORDER = ['items', 'title', 'priority', 'tags', 'tag']
const missing = (v: unknown) => v === undefined || v === null || v === ''

/**
 * class-validator's errors in Laravel's form (keys, wording, order by rule
 * and then element, summary). Errors in list elements (`each`) come without
 * an index, so it is taken from the checked list.
 */
function laravelErrors(errors: ValidationError[]): HttpException {
  const found: [number, number[], string, string][] = []
  const add = (rank: string, path: (string | number)[], msg: (k: string) => string) => {
    const key = path.join('.')
    found.push([ORDER.indexOf(rank), path.filter((p): p is number => typeof p === 'number'), key, msg(key)])
  }
  const walk = (es: ValidationError[], path: (string | number)[]) => {
    for (const e of es) {
      const prop: string | number = /^\d+$/.test(e.property) ? Number(e.property) : e.property
      const here = [...path, prop]
      const c = e.constraints ?? {}
      if (e.children?.length) walk(e.children, here)
      if (!Object.keys(c).length) continue
      if (prop === 'items') add('items', here, (k) => (missing(e.value) || c.arrayNotEmpty ? `The ${k} field is required.` : `The ${k} field must be an array.`))
      else if (prop === 'title') add('title', here, (k) => (missing(e.value) ? `The ${k} field is required.` : c.isString ? `The ${k} field must be a string.` : `The ${k} field must not be greater than 120 characters.`))
      else if (prop === 'priority') add('priority', here, (k) => (missing(e.value) ? `The ${k} field is required.` : c.isInt ? `The ${k} field must be an integer.` : `The ${k} field must be between 1 and 5.`))
      else if (prop === 'tags') {
        if (!Array.isArray(e.value)) add('tags', here, (k) => `The ${k} field must be an array.`)
        else e.value.forEach((v: unknown, j: number) => {
          if (typeof v !== 'string') add('tag', [...here, j], (k) => `The ${k} field must be a string.`)
          else if (v.length > 20) add('tag', [...here, j], (k) => `The ${k} field must not be greater than 20 characters.`)
        })
      }
    }
  }
  walk(errors, [])
  found.sort((a, b) => a[0] - b[0] || a[1].map((x, i) => x - (b[1][i] ?? -1)).find((d) => d !== 0) || 0)
  const out: Record<string, string[]> = {}
  for (const [, , key, msg] of found) (out[key] ??= []).push(msg)
  const all = Object.values(out).flat()
  const more = all.length - 1
  return new HttpException({ message: all[0] + (more > 0 ? ` (and ${more} more ${more === 1 ? 'error' : 'errors'})` : ''), errors: out }, 422)
}

const itemsPipe = new ValidationPipe({ transform: true, exceptionFactory: laravelErrors })

/**
 * Measurement tool (BENCH3, THESIS1): workloads with an adjustable amount of
 * application logic per request (`n`), the same steps and answers as the
 * Laravel app's BenchLogicController.
 */
@Controller('api/bench/logic')
@UseGuards(BenchTokenGuard)
export class BenchLogicController {
  constructor(private readonly db: DbService) {}

  private n(q: any): number {
    const v = Number.parseInt(q.n ?? '1', 10)
    return Math.min(1000, Math.max(0, Number.isNaN(v) ? 0 : v))
  }

  private rows(req: any, n: number): any[] {
    return this.db.select('SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?', [req.benchUser.id, n])
  }

  @Get('hydrate')
  hydrate(@Req() req: any, @Query() q: any) {
    return { data: this.rows(req, this.n(q)).map((r) => NoteView.fromRow(r).toArray()) }
  }

  @Get('collect')
  collect(@Req() req: any, @Query() q: any) {
    const mapped = this.rows(req, this.n(q))
      .map((r) => ({ title: r.title, priority: r.priority, done: r.done === 1, chars: r.title.length + String(r.body ?? '').length }))
      .filter((r) => !r.done)
    const groups = Map.groupBy(mapped, (r) => r.priority)
    const out: Record<number, any> = {}
    for (const key of [...groups.keys()].sort((a, b) => a - b)) {
      const g = groups.get(key)!
      const first = [...g].sort((a, b) => (a.title < b.title ? -1 : a.title > b.title ? 1 : 0))[0]
      out[key] = { count: g.length, chars: g.reduce((s, r) => s + r.chars, 0), first: first.title }
    }
    return { groups: out }
  }

  @Post('validate')
  @HttpCode(200)
  validate(@Body(itemsPipe) data: ItemsDto) {
    return { valid: data.items.length }
  }

  @Get('json')
  json(@Query() q: any) {
    const n = this.n(q)
    const items = []
    for (let i = 0; i < n; i++) items.push({ id: i, name: 'item ' + i, tags: ['a', 'b', 'c'], meta: { created: '2026-10-27', score: i * 1.5, flags: { x: true, y: false } } })
    return { count: n, items }
  }
}
