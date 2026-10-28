import { json, logicN, logicRows, withToken } from '@/lib/bench'

/** n notes prepared with the language's arrays: map, filter, groupBy, sortBy, sum. */
export const GET = withToken((req, user) => {
  const mapped = logicRows(user, logicN(req))
    .map((r) => ({ title: r.title as string, priority: r.priority as number, done: r.done === 1, chars: r.title.length + String(r.body ?? '').length }))
    .filter((r) => !r.done)
  const groups = Map.groupBy(mapped, (r) => r.priority)
  const out: Record<number, unknown> = {}
  for (const key of [...groups.keys()].sort((a, b) => a - b)) {
    const g = groups.get(key)!
    const first = [...g].sort((a, b) => (a.title < b.title ? -1 : a.title > b.title ? 1 : 0))[0]
    out[key] = { count: g.length, chars: g.reduce((s, r) => s + r.chars, 0), first: first.title }
  }
  return json({ groups: out })
})
