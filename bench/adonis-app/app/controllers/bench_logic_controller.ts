import type { HttpContext } from '@adonisjs/core/http'
import { rawQuery } from '#bench/db'
import vine from '@vinejs/vine'
import { NoteView } from '#bench/views'
import { LaravelErrorReporter } from '#bench/laravel_errors'

const validator = vine.create(
  vine.object({
    // Laravel's `required` fails on an empty list too (and on "", which the
    // body parser turns into null)
    items: vine
      .array(
        vine.object({
          title: vine.string().maxLength(120),
          priority: vine.number().withoutDecimals().range([1, 5]),
          tags: vine.array(vine.string().maxLength(20)).optional(),
        })
      )
      .minLength(1),
  })
)

/**
 * Measurement tool (BENCH3, THESIS1): workloads with an adjustable amount of
 * application logic per request (`n`), the same steps and answers as the
 * Laravel app's BenchLogicController, on the framework-core basis.
 */
export default class BenchLogicController {
  private n(ctx: HttpContext): number {
    const v = Number.parseInt(ctx.request.qs().n ?? '1', 10)
    return Math.min(1000, Math.max(0, Number.isNaN(v) ? 0 : v))
  }

  private async rows(ctx: HttpContext, n: number): Promise<any[]> {
    return rawQuery('SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?', [(ctx as any).benchUser.id, n])
  }

  /** n notes with their user, hydrated into objects and listed. */
  async hydrate(ctx: HttpContext) {
    const out = []
    for (const row of await this.rows(ctx, this.n(ctx))) {
      out.push(NoteView.fromRow(row).toArray())
    }
    return ctx.response.json({ data: out })
  }

  /** n notes prepared with the language's arrays: map, filter, groupBy, sortBy, sum. */
  async collect(ctx: HttpContext) {
    const mapped = (await this.rows(ctx, this.n(ctx)))
      .map((r) => ({ title: r.title, priority: r.priority, done: r.done === 1, chars: r.title.length + String(r.body ?? '').length }))
      .filter((r) => !r.done)
    const groups = Map.groupBy(mapped, (r) => r.priority)
    const out: Record<number, any> = {}
    for (const key of [...groups.keys()].sort((a, b) => a - b)) {
      const g = groups.get(key)!
      const first = [...g].sort((a, b) => (a.title < b.title ? -1 : a.title > b.title ? 1 : 0))[0]
      out[key] = { count: g.length, chars: g.reduce((s, r) => s + r.chars, 0), first: first.title }
    }
    return ctx.response.json({ groups: out })
  }

  /** A nested input of n items, validated; answers the number of items. */
  async validate(ctx: HttpContext) {
    try {
      const data = await validator.validate(ctx.request.body(), { errorReporter: () => new LaravelErrorReporter() })
      return ctx.response.json({ valid: data.items.length })
    } catch (error: any) {
      if (error.laravel) return ctx.response.status(422).json(error.laravel)
      throw error
    }
  }

  /** A nested structure of depth 3 with n items, no database. */
  json(ctx: HttpContext) {
    const n = this.n(ctx)
    const items = []
    for (let i = 0; i < n; i++) {
      items.push({ id: i, name: 'item ' + i, tags: ['a', 'b', 'c'], meta: { created: '2026-10-27', score: i * 1.5, flags: { x: true, y: false } } })
    }
    return ctx.response.json({ count: n, items })
  }
}
