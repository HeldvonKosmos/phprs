import { z } from 'zod'
import { json, withToken } from '@/lib/bench'
import { laravelErrors } from '@/lib/laravel-errors'

// Laravel's `required` fails on "" and on an empty list too
const Items = z.object({
  items: z
    .array(
      z.object({
        title: z.string().min(1).max(120),
        priority: z.int().min(1).max(5),
        tags: z.array(z.string().max(20)).optional(),
      })
    )
    .min(1),
})

/** A nested input of n items, validated; answers the number of items. */
export const POST = withToken(async (req) => {
  const data = await req.json().catch(() => ({}))
  const result = Items.safeParse(data, { reportInput: true })
  if (!result.success) return json(laravelErrors(result.error.issues), 422)
  return json({ valid: result.data.items.length })
})
