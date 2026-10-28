import { createHash, randomBytes } from 'node:crypto'
import bcrypt from 'bcrypt'
import { conn, json, now } from '@/lib/bench'

export async function POST(req: Request) {
  const body: any = await req.json().catch(() => ({}))
  const { email, password } = body ?? {}
  if (typeof email !== 'string' || typeof password !== 'string') return json({ message: 'The email and password fields are required.' }, 422)
  const row: any = conn().prepare('SELECT id, password FROM users WHERE email = ? LIMIT 1').get(email)
  if (!row || !(await bcrypt.compare(password, row.password))) return json({ message: 'Invalid credentials.' }, 401)
  const token = randomBytes(32).toString('hex')
  conn().prepare('INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)').run(row.id, createHash('sha256').update(token).digest('hex'), Math.floor(Date.now() / 1000), now())
  return json({ token })
}
