import { json, logicN, logicRows, withToken } from '@/lib/bench'
import { NoteView } from '@/lib/views'

/** n notes with their user, hydrated into objects and listed. */
export const GET = withToken((req, user) => json({ data: logicRows(user, logicN(req)).map((r) => NoteView.fromRow(r).toArray()) }))
