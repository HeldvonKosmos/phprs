<?php

namespace App\Http\Controllers;

use App\Bench\NoteView;
use Illuminate\Http\JsonResponse;
use Illuminate\Http\Request;
use Illuminate\Support\Collection;
use Illuminate\Support\Facades\DB;

/**
 * Measurement tool (THESIS1): workloads with an adjustable amount of
 * application logic per request (`n`), the same steps and answers as
 * OpenGrid's BenchLogicController, on the framework-core basis (D23).
 */
class BenchLogicController
{
    public function __construct(private Request $request)
    {
    }

    private function n(): int
    {
        return min(1000, max(0, (int) $this->request->query('n', '1')));
    }

    private function rows(int $n): array
    {
        return DB::connection('bench_sqlite')->select('SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?', [$this->request->user()['id'], $n]);
    }

    /** n notes with their user, hydrated into objects and listed. */
    public function hydrate(): JsonResponse
    {
        $out = [];
        foreach ($this->rows($this->n()) as $row) {
            $out[] = NoteView::fromRow($row)->toArray();
        }

        return response()->json(['data' => $out]);
    }

    /** n notes prepared with the collection: map, filter, groupBy, sortBy, sum. */
    public function collect(): JsonResponse
    {
        $groups = Collection::make($this->rows($this->n()))
            ->map(fn (object $r): array => ['title' => $r->title, 'priority' => $r->priority, 'done' => $r->done === 1, 'chars' => strlen($r->title) + strlen((string) $r->body)])
            ->filter(fn (array $r): bool => ! $r['done'])
            ->groupBy('priority')
            ->map(fn (Collection $g): array => ['count' => $g->count(), 'chars' => $g->sum('chars'), 'first' => $g->sortBy('title')->first()['title']])
            ->sortKeys();

        return response()->json(['groups' => $groups->all()]);
    }

    /** A nested input of n items, validated; answers the number of items. */
    public function validate(): JsonResponse
    {
        $data = $this->request->validate([
            'items' => 'required|array',
            'items.*.title' => 'required|string|max:120',
            'items.*.priority' => 'required|integer|between:1,5',
            'items.*.tags' => 'array',
            'items.*.tags.*' => 'string|max:20',
        ]);

        return response()->json(['valid' => count($data['items'])]);
    }

    /** A nested structure of depth 3 with n items, no database. */
    public function json(): JsonResponse
    {
        $n = $this->n();
        $items = [];
        for ($i = 0; $i < $n; $i++) {
            $items[] = ['id' => $i, 'name' => 'item '.$i, 'tags' => ['a', 'b', 'c'], 'meta' => ['created' => '2026-10-27', 'score' => $i * 1.5, 'flags' => ['x' => true, 'y' => false]]];
        }

        return response()->json(['count' => $n, 'items' => $items]);
    }
}
