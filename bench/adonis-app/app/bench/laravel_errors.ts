import type { ErrorReporterContract, FieldContext } from '@vinejs/vine/types'

/**
 * Measurement tool (BENCH3): VineJS errors in Laravel's form, so that every
 * side answers the validate workload byte for byte the same: keys such as
 * items.0.title, Laravel's wording, ordered by rule and then by element (as
 * Laravel expands its wildcard rules), the first message with
 * "(and N more errors)" as `message`.
 */
const ORDER = [/^items$/, /^items\.\d+\.title$/, /^items\.\d+\.priority$/, /^items\.\d+\.tags$/, /^items\.\d+\.tags\.\d+$/]

function laravelMessage(field: string, rule: string, meta: any): string {
  switch (rule) {
    case 'required':
    case 'array.minLength':
    case 'minLength':
      return `The ${field} field is required.`
    case 'string':
      return `The ${field} field must be a string.`
    case 'maxLength':
      return `The ${field} field must not be greater than ${meta?.max} characters.`
    case 'number':
    case 'withoutDecimals':
      return `The ${field} field must be an integer.`
    case 'range':
      return `The ${field} field must be between ${meta?.min} and ${meta?.max}.`
    case 'array':
      return `The ${field} field must be an array.`
    default:
      return `The ${field} field is invalid.`
  }
}

export class LaravelErrorReporter implements ErrorReporterContract {
  hasErrors = false
  errors: { field: string; message: string }[] = []

  report(_message: string, rule: string, field: FieldContext, meta?: any) {
    this.hasErrors = true
    const name = field.getFieldPath()
    this.errors.push({ field: name, message: laravelMessage(name, rule, meta) })
  }

  createError() {
    const rank = (f: string) => ORDER.findIndex((r) => r.test(f))
    const idx = (f: string) => (f.match(/\d+/g) ?? []).map(Number)
    const sorted = [...this.errors].sort((a, b) => {
      const d = rank(a.field) - rank(b.field)
      if (d !== 0) return d
      const ia = idx(a.field), ib = idx(b.field)
      for (let i = 0; i < Math.max(ia.length, ib.length); i++) {
        if ((ia[i] ?? -1) !== (ib[i] ?? -1)) return (ia[i] ?? -1) - (ib[i] ?? -1)
      }
      return 0
    })
    const errors: Record<string, string[]> = {}
    for (const e of sorted) (errors[e.field] ??= []).push(e.message)
    const all = Object.values(errors).flat()
    const more = all.length - 1
    const message = all[0] + (more > 0 ? ` (and ${more} more ${more === 1 ? 'error' : 'errors'})` : '')
    const err: any = new Error(message)
    err.status = 422
    err.laravel = { message, errors }
    return err
  }
}
