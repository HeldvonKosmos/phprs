<?php

namespace App\Http\Middleware;

use Closure;
use Illuminate\Http\Request;
use Illuminate\Support\Facades\DB;

/**
 * Measurement tool (D23): the token check of the framework-core benchmark,
 * the same statements as in OpenGrid's benchsql module. The token's use is
 * written at most once a second, as Sanctum does in effect.
 */
class BenchToken
{
    public function handle(Request $request, Closure $next): mixed
    {
        $token = $request->bearerToken();
        if (! is_string($token) || $token === '') {
            return response()->json(['message' => 'Unauthenticated.'], 401);
        }
        $db = DB::connection('bench_sqlite');
        $rows = $db->select('SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1', [hash('sha256', $token)]);
        if (count($rows) === 0) {
            return response()->json(['message' => 'Unauthenticated.'], 401);
        }
        $row = $rows[0];
        $now = time();
        if ($now - $row->last_used_at >= 1) {
            $db->update('UPDATE tokens SET last_used_at = ? WHERE id = ?', [$now, $row->token_id]);
        }
        $user = ['id' => $row->id, 'name' => $row->name, 'email' => $row->email];
        $request->setUserResolver(fn () => $user);

        return $next($request);
    }
}
