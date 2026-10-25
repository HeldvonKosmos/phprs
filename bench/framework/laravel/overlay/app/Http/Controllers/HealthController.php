<?php

namespace App\Http\Controllers;

use Illuminate\Http\JsonResponse;
use Illuminate\Support\Facades\DB;

/** GET /health, the same shape as OpenGrid's. */
class HealthController
{
    public function __invoke(): JsonResponse
    {
        $connection = DB::connection();
        $driver = $connection->getDriverName();
        $version = $driver === 'sqlite' ? $connection->selectOne('select sqlite_version() as v')->v : (string) $connection->getServerVersion();

        return response()->json([
            'status' => 'ok',
            'app' => config('app.name'),
            'database' => ['mode' => $driver, 'version' => $version],
        ]);
    }
}
