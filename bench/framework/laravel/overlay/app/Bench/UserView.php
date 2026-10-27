<?php

namespace App\Bench;

/** Measurement tool (THESIS1): the user of a hydrated note. */
final class UserView
{
    public function __construct(public int $id, public string $name, public string $email)
    {
    }

    public function toArray(): array
    {
        return ['id' => $this->id, 'name' => $this->name, 'email' => $this->email];
    }
}
