import { conn, json, note, now, withToken } from '@/lib/bench'

/** GET /api/bench/sql/notes?done=false&per_page=15&page=1 */
export const GET = withToken((req, user) => {
  const q = new URL(req.url).searchParams
  const done = (q.get('done') ?? 'false') === 'true' ? 1 : 0
  const perPage = Number.parseInt(q.get('per_page') ?? '15', 10) || 0
  const page = Number.parseInt(q.get('page') ?? '1', 10) || 0
  const total = (conn().prepare('SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL').get(user.id, done) as any).total
  const rows = conn().prepare('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?').all(user.id, done, perPage, (page - 1) * perPage)
  return json({ data: rows.map(note), total, per_page: perPage, current_page: page })
})

export const POST = withToken(async (req, user) => {
  const body: any = await req.json().catch(() => ({}))
  const title = body?.title
  if (typeof title !== 'string' || title === '') return json({ message: 'The title field is required.' }, 422)
  const text = typeof body.body === 'string' ? body.body : null
  const priority = Number.isInteger(body.priority ?? 3) ? (body.priority ?? 3) : 3
  const t = now()
  const id = (conn().prepare('INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id').get(user.id, title, text, priority, t, t) as any).id
  return json({ data: { id, title, body: text, done: false, priority, created_at: t, updated_at: t } }, 201)
})
