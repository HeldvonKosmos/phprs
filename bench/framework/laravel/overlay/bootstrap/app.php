<?php

use App\Http\Controllers\HealthController;
use Illuminate\Auth\AuthenticationException;
use Illuminate\Foundation\Application;
use Illuminate\Foundation\Configuration\Exceptions;
use Illuminate\Validation\ValidationException;
use Illuminate\Foundation\Configuration\Middleware;
use Illuminate\Support\Facades\Route;
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
        // JSON for every validation error, also without "Accept: application/json"
        // (api.sh and oha send none), in exactly Laravel's own JSON form
        // (Handler::invalidJson: the summarised message and the errors)
        $exceptions->render(function (ValidationException $e) {
            return response()->json(['message' => $e->getMessage(), 'errors' => $e->errors()], $e->status);
        });
        $exceptions->render(function (HttpExceptionInterface $e) {
            return response()->json(['error' => 'HttpException', 'message' => $e->getMessage()], $e->getStatusCode(), $e->getHeaders());
        });
    })->create();
