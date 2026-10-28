import { json, logicN, withToken } from '@/lib/bench'

/** A nested structure of depth 3 with n items, no database. */
export const GET = withToken((req) => {
  const n = logicN(req)
  const items = []
  for (let i = 0; i < n; i++) items.push({ id: i, name: 'item ' + i, tags: ['a', 'b', 'c'], meta: { created: '2026-10-27', score: i * 1.5, flags: { x: true, y: false } } })
  return json({ count: n, items })
})
