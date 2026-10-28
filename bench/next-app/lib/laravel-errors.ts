import type { z } from 'zod'

/**
 * Measurement tool (BENCH3): zod's issues in Laravel's form (keys such as
 * items.0.title, Laravel's wording, ordered by rule and then by element, the
 * first message with "(and N more errors)"), so every side answers alike.
 */
const ORDER = ['items', 'title', 'priority', 'tags', 'tag']

export function laravelErrors(issues: z.core.$ZodIssue[]) {
  const found: [number, number[], string, string][] = []
  for (const i of issues) {
    const path = i.path as (string | number)[]
    const key = path.join('.')
    const last = path[path.length - 1]
    const rank = path.length === 1 ? 'items' : typeof last === 'number' ? 'tag' : String(last)
    const input = (i as any).input
    const missing = input === undefined || input === null
    let msg: string
    if (rank === 'items') msg = missing || i.code === 'too_small' ? `The ${key} field is required.` : `The ${key} field must be an array.`
    else if (rank === 'title') msg = missing || i.code === 'too_small' ? `The ${key} field is required.` : i.code === 'too_big' ? `The ${key} field must not be greater than 120 characters.` : `The ${key} field must be a string.`
    else if (rank === 'priority') msg = missing ? `The ${key} field is required.` : i.code === 'invalid_type' ? `The ${key} field must be an integer.` : `The ${key} field must be between 1 and 5.`
    else if (rank === 'tags') msg = `The ${key} field must be an array.`
    else msg = i.code === 'too_big' ? `The ${key} field must not be greater than 20 characters.` : `The ${key} field must be a string.`
    found.push([ORDER.indexOf(rank), path.filter((p): p is number => typeof p === 'number'), key, msg])
  }
  found.sort((a, b) => a[0] - b[0] || a[1].map((x, j) => x - (b[1][j] ?? -1)).find((d) => d !== 0) || 0)
  const errors: Record<string, string[]> = {}
  for (const [, , key, msg] of found) (errors[key] ??= []).push(msg)
  const all = Object.values(errors).flat()
  const more = all.length - 1
  return { message: all[0] + (more > 0 ? ` (and ${more} more ${more === 1 ? 'error' : 'errors'})` : ''), errors }
}
