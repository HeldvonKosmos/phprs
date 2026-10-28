import { json, sqliteInfo } from '@/lib/bench'

/** The SQLite this side runs (version and compile options), for the header. */
export async function GET() {
  return json(sqliteInfo())
}
