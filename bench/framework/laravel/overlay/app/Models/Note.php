<?php

namespace App\Models;

use App\Models\Concerns\HasRecordId;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\SoftDeletes;

/** A note of one user; deleting keeps the record (soft delete). */
class Note extends Model
{
    use HasRecordId, SoftDeletes;

    protected $fillable = ['user', 'title', 'body', 'done', 'priority'];

    protected $hidden = ['deleted_at'];

    protected $dateFormat = 'Y-m-d H:i:s.u';

    protected $attributes = ['done' => false, 'priority' => 3];

    /** Fields allowed in ?filter[…], with the type the query string value is cast to. */
    public const FILTERABLE = ['done' => 'bool', 'priority' => 'int'];

    /** Fields allowed in ?sort=. */
    public const SORTABLE = ['title', 'body', 'done', 'priority', 'created_at', 'updated_at'];

    protected function casts(): array
    {
        return ['done' => 'boolean', 'priority' => 'integer'];
    }
}
