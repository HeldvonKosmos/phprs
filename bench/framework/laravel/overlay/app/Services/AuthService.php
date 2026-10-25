<?php

namespace App\Services;

use App\Models\User;
use Illuminate\Support\Facades\Hash;

/** Registration and token login; injected into the controller. */
class AuthService
{
    /** A valid bcrypt hash, checked for unknown emails so both cases take the same time. */
    private const DUMMY_HASH = '$2y$10$c9I5e50pSO/sY8GirDR4iex0UUR5X/smTgxo2402ubMbAGHcxsVI.';

    public function register(string $name, string $email, string $password): User
    {
        return User::create(['name' => $name, 'email' => $email, 'password' => Hash::make($password)]);
    }

    /** `['token' => …, 'user' => …]`, or null for unknown email and wrong password alike. */
    public function login(string $email, string $password): ?array
    {
        $user = User::where('email', $email)->first();
        $ok = Hash::check($password, $user?->password ?? self::DUMMY_HASH);
        if (! $ok || $user === null) {
            return null;
        }

        return ['token' => $user->createApiToken(), 'user' => $user];
    }

    public function logout(User $user): void
    {
        $user->currentAccessToken()->delete();
    }
}
