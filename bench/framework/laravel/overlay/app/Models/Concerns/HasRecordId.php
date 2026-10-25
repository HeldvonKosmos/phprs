<?php

namespace App\Models\Concerns;

use Illuminate\Support\Str;

/**
 * Record ids like SurrealDB (`notes:k3x…`), so both applications answer with
 * the same id format; the part after the colon goes into the URL.
 */
trait HasRecordId
{
    public static function bootHasRecordId(): void
    {
        static::creating(function ($model) {
            if (! $model->getKey()) {
                $model->setAttribute($model->getKeyName(), $model->getTable().':'.Str::lower(Str::random(20)));
            }
        });
    }

    public function initializeHasRecordId(): void
    {
        $this->incrementing = false;
        $this->keyType = 'string';
    }

    /** Timestamps as in OpenGrid: ISO 8601 with microseconds and offset. */
    protected function serializeDate(\DateTimeInterface $date): string
    {
        return $date->format('Y-m-d\TH:i:s.uP');
    }

    public static function findRecord(string $id): ?static
    {
        return static::find((new static)->getTable().':'.$id);
    }
}
