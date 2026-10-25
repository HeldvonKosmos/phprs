<?php

namespace App\Http\Controllers;

use App\Services\AuthService;
use Illuminate\Http\JsonResponse;
use Illuminate\Http\Request;
use Illuminate\Http\Response;

class AuthController
{
    public function __construct(private AuthService $auth)
    {
    }

    /** POST /api/register */
    public function register(Request $request): JsonResponse
    {
        $data = $request->validate([
            'name' => 'required|string|max:120',
            'email' => 'required|email|unique:users,email',
            'password' => 'required|string|min:8|confirmed',
        ]);

        return response()->json($this->auth->register($data['name'], $data['email'], $data['password']), 201);
    }

    /** POST /api/login */
    public function login(Request $request): JsonResponse
    {
        $data = $request->validate(['email' => 'required|string', 'password' => 'required|string']);
        $result = $this->auth->login($data['email'], $data['password']);
        if ($result === null) {
            abort(401, 'These credentials do not match our records.');
        }

        return response()->json($result);
    }

    /** POST /api/logout (auth) */
    public function logout(Request $request): Response
    {
        $this->auth->logout($request->user());

        return response()->noContent();
    }

    /** GET /api/me (auth) */
    public function me(Request $request): JsonResponse
    {
        return response()->json($request->user());
    }
}
