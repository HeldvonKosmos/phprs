<?php

namespace App\Http\Controllers;

use Illuminate\Http\JsonResponse;
use Illuminate\Http\Request;
use Illuminate\Support\Facades\DB;

/**
 * Measurement tool (D23): the framework-core benchmark with raw SQL on
 * SQLite. OpenGrid's benchsql module has the same controller with the same
 * statements and the same answers.
 */
class BenchSqlController
{
    public function __construct(private Request $request)
    {
    }

    public function health(): JsonResponse
    {
        return response()->json(['status' => 'ok']);
    }

    /** The SQLite this side runs and the runtime, for the measurement header (BENCH3). */
    public function sqlite(): JsonResponse
    {
        $db = DB::connection('bench_sqlite');
        $options = array_map(fn ($row) => $row->compile_options, $db->select('PRAGMA compile_options'));
        return response()->json(['version' => $db->select('SELECT sqlite_version() AS v')[0]->v, 'compile_options' => $options, 'runtime' => ['php' => PHP_VERSION, 'sapi' => PHP_SAPI, 'laravel' => app()->version(), 'opcache_jit' => ini_get('opcache.jit'), 'opcache_validate_timestamps' => ini_get('opcache.validate_timestamps'), 'opcache_preload' => ini_get('opcache.preload') !== '' && ini_get('opcache.preload') !== false]]);
    }

    public function login(): JsonResponse
    {
        $email = $this->request->input('email');
        $password = $this->request->input('password');
        if (! is_string($email) || ! is_string($password)) {
            return response()->json(['message' => 'The email and password fields are required.'], 422);
        }
        $db = DB::connection('bench_sqlite');
        $rows = $db->select('SELECT id, password FROM users WHERE email = ? LIMIT 1', [$email]);
        if (count($rows) === 0 || ! password_verify($password, $rows[0]->password)) {
            return response()->json(['message' => 'Invalid credentials.'], 401);
        }
        $token = bin2hex(random_bytes(32));
        $db->insert('INSERT INTO tokens (user_id, token_hash, last_used_at, created_at) VALUES (?, ?, ?, ?)', [$rows[0]->id, hash('sha256', $token), time(), gmdate('Y-m-d H:i:s')]);

        return response()->json(['token' => $token]);
    }

    /** GET /api/bench/sql/notes?done=false&per_page=15&page=1 */
    public function index(): JsonResponse
    {
        $user = $this->request->user();
        $done = $this->request->query('done', 'false') === 'true' ? 1 : 0;
        $perPage = (int) $this->request->query('per_page', '15');
        $page = (int) $this->request->query('page', '1');
        $db = DB::connection('bench_sqlite');
        $total = $db->select('SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL', [$user['id'], $done])[0]->total;
        $rows = $db->select('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?', [$user['id'], $done, $perPage, ($page - 1) * $perPage]);
        $data = [];
        foreach ($rows as $row) {
            $data[] = $this->note($row);
        }

        return response()->json(['data' => $data, 'total' => $total, 'per_page' => $perPage, 'current_page' => $page]);
    }

    public function show(string $id): JsonResponse
    {
        $user = $this->request->user();
        $rows = DB::connection('bench_sqlite')->select('SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1', [(int) $id, $user['id']]);
        if (count($rows) === 0) {
            return response()->json(['message' => 'Not found.'], 404);
        }

        return response()->json(['data' => $this->note($rows[0])]);
    }

    public function store(): JsonResponse
    {
        $user = $this->request->user();
        $title = $this->request->input('title');
        if (! is_string($title) || $title === '') {
            return response()->json(['message' => 'The title field is required.'], 422);
        }
        $body = $this->request->input('body');
        $priority = $this->request->input('priority', 3);
        $now = gmdate('Y-m-d H:i:s');
        $id = DB::connection('bench_sqlite')->select('INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id', [$user['id'], $title, is_string($body) ? $body : null, is_int($priority) ? $priority : 3, $now, $now])[0]->id;

        return response()->json(['data' => ['id' => $id, 'title' => $title, 'body' => is_string($body) ? $body : null, 'done' => false, 'priority' => is_int($priority) ? $priority : 3, 'created_at' => $now, 'updated_at' => $now]], 201);
    }

    private function note(object $row): array
    {
        return ['id' => $row->id, 'title' => $row->title, 'body' => $row->body, 'done' => $row->done === 1, 'priority' => $row->priority, 'created_at' => $row->created_at, 'updated_at' => $row->updated_at];
    }
}
