<?php

use Illuminate\Database\Migrations\Migration;
use Illuminate\Database\Schema\Blueprint;
use Illuminate\Support\Facades\Schema;

// The TP5 schema: users, Sanctum tokens (tokenable id is a record id string), notes.
return new class extends Migration
{
    public function up(): void
    {
        Schema::create('users', function (Blueprint $table) {
            $table->string('id', 40)->primary();
            $table->string('name');
            $table->string('email')->unique();
            $table->string('password');
            $table->rememberToken();
            $table->timestamps(6);
        });

        Schema::create('personal_access_tokens', function (Blueprint $table) {
            $table->id();
            $table->string('tokenable_type');
            $table->string('tokenable_id', 40);
            $table->text('name');
            $table->string('token', 64)->unique();
            $table->text('abilities')->nullable();
            $table->timestamp('last_used_at')->nullable();
            $table->timestamp('expires_at')->nullable()->index();
            $table->timestamps();
            $table->index(['tokenable_type', 'tokenable_id']);
        });

        Schema::create('notes', function (Blueprint $table) {
            $table->string('id', 40)->primary();
            $table->string('user', 40)->index();
            $table->string('title', 120);
            $table->text('body')->nullable();
            $table->boolean('done')->default(false)->index();
            $table->integer('priority')->default(3);
            $table->timestamps(6);
            $table->softDeletes('deleted_at', 6);
            // the same composite indexes as the OpenGrid test app (DBP1 M3):
            // list filtered by done and sorted by priority, and unfiltered
            $table->index(['user', 'done', 'deleted_at', 'priority']);
            $table->index(['user', 'deleted_at', 'priority']);
        });
    }

    public function down(): void
    {
        Schema::dropIfExists('notes');
        Schema::dropIfExists('personal_access_tokens');
        Schema::dropIfExists('users');
    }
};
