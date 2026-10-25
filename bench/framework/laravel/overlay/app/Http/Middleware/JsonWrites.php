<?php

namespace App\Http\Middleware;

use Closure;
use Illuminate\Http\Request;

/** A request with a body must send JSON; every answer names the module. */
class JsonWrites
{
    public function handle(Request $request, Closure $next): mixed
    {
        if ($request->getContent() !== '' && ! $request->isJson()) {
            abort(415, 'send application/json');
        }
        $response = $next($request);
        $response->headers->set('X-Module', 'notes');

        return $response;
    }
}
