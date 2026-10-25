<?php

namespace App\Models;

use App\Models\Concerns\HasRecordId;
use Illuminate\Foundation\Auth\User as Authenticatable;
use Laravel\Sanctum\HasApiTokens;

class User extends Authenticatable
{
    use HasApiTokens, HasRecordId;

    protected $fillable = ['name', 'email', 'password'];

    protected $hidden = ['password', 'remember_token'];

    protected $dateFormat = 'Y-m-d H:i:s.u';

    /**
     * A bearer token like OpenGrid's: 256 random bits as 64 hex characters,
     * stored only as SHA-256. Sanctum finds a token without `id|` prefix by
     * its hash, so the guard needs no change.
     */
    public function createApiToken(string $name = 'api'): string
    {
        $plain = bin2hex(random_bytes(32));
        $this->tokens()->create(['name' => $name, 'token' => hash('sha256', $plain), 'abilities' => ['*']]);

        return $plain;
    }
}
