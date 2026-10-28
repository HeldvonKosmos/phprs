<?php

use App\Http\Controllers\AuthController;
use App\Http\Controllers\BenchLogicController;
use App\Http\Controllers\BenchSqlController;
use App\Http\Controllers\NoteController;
use App\Http\Middleware\BenchToken;
use App\Http\Middleware\JsonWrites;
use Illuminate\Support\Facades\Route;

Route::post('/register', [AuthController::class, 'register']);
Route::post('/login', [AuthController::class, 'login']);
Route::middleware('auth:sanctum')->group(function () {
    Route::post('/logout', [AuthController::class, 'logout']);
    Route::get('/me', [AuthController::class, 'me']);
});

Route::prefix('notes')->middleware(['auth:sanctum', JsonWrites::class])->group(function () {
    Route::get('', [NoteController::class, 'index']);
    Route::post('', [NoteController::class, 'store']);
    Route::get('/{id}', [NoteController::class, 'show']);
    Route::patch('/{id}', [NoteController::class, 'update']);
    Route::delete('/{id}', [NoteController::class, 'destroy']);
});

// Measurement tool (D23, DBR1): the framework-core benchmark. Both apps run
// exactly these raw SQL statements against the same SQLite database; ORM and
// auth are replaced by them on both sides on purpose.
Route::prefix('bench/sql')->group(function () {
    Route::get('/health', [BenchSqlController::class, 'health']);
    Route::get('/sqlite', [BenchSqlController::class, 'sqlite']);
    Route::post('/login', [BenchSqlController::class, 'login']);
});
Route::prefix('bench/sql/notes')->middleware(BenchToken::class)->group(function () {
    Route::get('', [BenchSqlController::class, 'index']);
    Route::post('', [BenchSqlController::class, 'store']);
    Route::get('/{id}', [BenchSqlController::class, 'show']);
});

// THESIS1: workloads with an adjustable amount of logic per request (?n=)
Route::prefix('bench/logic')->middleware(BenchToken::class)->group(function () {
    Route::get('/hydrate', [BenchLogicController::class, 'hydrate']);
    Route::get('/collect', [BenchLogicController::class, 'collect']);
    Route::post('/validate', [BenchLogicController::class, 'validate']);
    Route::get('/json', [BenchLogicController::class, 'json']);
});
