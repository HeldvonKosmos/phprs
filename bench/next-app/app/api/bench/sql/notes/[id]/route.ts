import { conn, json, note, withToken } from '@/lib/bench'

export const GET = withToken(async (_req, user, ctx: { params: Promise<{ id: string }> }) => {
  const { id } = await ctx.params
  const row = conn().prepare('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1').get(Number.parseInt(id, 10) || 0, user.id)
  if (!row) return json({ message: 'Not found.' }, 404)
  return json({ data: note(row) })
})
