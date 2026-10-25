<?php

use App\Http\Controllers\HealthController;
use Illuminate\Auth\AuthenticationException;
use Illuminate\Foundation\Application;
use Illuminate\Foundation\Configuration\Exceptions;
use Illuminate\Foundation\Configuration\Middleware;
use Illuminate\Support\Facades\Route;
use Illuminate\Validation\ValidationException;
use Symfony\Component\HttpKernel\Exception\HttpExceptionInterface;

// Error bodies as in OpenGrid: {"error":"HttpException","message":…}; validation
// {"message": first error, "errors": {…}}.
return Application::configure(basePath: dirname(__DIR__))
    ->withRouting(
        api: __DIR__.'/../routes/api.php',
        commands: __DIR__.'/../routes/console.php',
        then: function () {
            Route::get('/health', HealthController::class);
        },
    )
    ->withMiddleware(function (Middleware $middleware): void {
        // an API without a login page: unauthenticated requests get 401, no redirect
        $middleware->redirectGuestsTo(fn () => null);
    })
    ->withExceptions(function (Exceptions $exceptions): void {
        $exceptions->render(function (AuthenticationException $e) {
            return response()->json(['error' => 'HttpException', 'message' => 'Unauthenticated.'], 401);
        });
        $exceptions->render(function (ValidationException $e) {
            $errors = $e->errors();

            return response()->json(['message' => reset($errors)[0], 'errors' => $errors], 422);
        });
        $exceptions->render(function (HttpExceptionInterface $e) {
            return response()->json(['error' => 'HttpException', 'message' => $e->getMessage()], $e->getStatusCode(), $e->getHeaders());
        });
    })->create();
