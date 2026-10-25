<?php

namespace App\Http\Controllers;

use App\Models\Note;
use Illuminate\Database\Eloquent\Builder;
use Illuminate\Http\JsonResponse;
use Illuminate\Http\Request;
use Illuminate\Http\Response;

/** The signed-in user's notes; another user's note answers 404. */
class NoteController
{
    /** GET /api/notes?filter[done]=false&sort=priority:desc&page=1&per_page=15 */
    public function index(Request $request): JsonResponse
    {
        $query = Note::where('user', $request->user()->id);
        $filters = $request->query('filter', []);
        if (is_array($filters)) {
            $this->filter($query, $filters);
        }
        $this->sort($query, (string) $request->query('sort', ''));
        $perPage = max(1, (int) $request->query('per_page', '15'));
        $page = max(1, (int) $request->query('page', '1'));
        $p = $query->paginate($perPage, ['*'], 'page', $page);

        return response()->json([
            'data' => $p->items(),
            'total' => $p->total(),
            'per_page' => $p->perPage(),
            'current_page' => $p->currentPage(),
            'last_page' => $p->lastPage(),
        ]);
    }

    public function store(Request $request): JsonResponse
    {
        $data = $request->validate([
            'title' => 'required|string|max:120',
            'body' => 'nullable|string',
            'done' => 'nullable|boolean',
            'priority' => 'nullable|integer|between:1,5',
        ]);
        $data['user'] = $request->user()->id;
        $note = Note::create(array_filter($data, fn ($v) => $v !== null));

        return response()->json($note, 201);
    }

    public function show(Request $request, string $id): JsonResponse
    {
        return response()->json($this->owned($request, $id));
    }

    public function update(Request $request, string $id): JsonResponse
    {
        $note = $this->owned($request, $id);
        $data = $request->validate([
            'title' => 'sometimes|string|max:120',
            'body' => 'nullable|string',
            'done' => 'sometimes|boolean',
            'priority' => 'sometimes|integer|between:1,5',
        ]);
        $note->update($data);

        return response()->json($note);
    }

    public function destroy(Request $request, string $id): Response
    {
        $this->owned($request, $id)->delete();

        return response()->noContent();
    }

    /** The note if it belongs to the signed-in user; 404 otherwise (no hint that a foreign id exists). */
    private function owned(Request $request, string $id): Note
    {
        $note = Note::findRecord($id);
        if ($note === null || $note->user !== $request->user()->id) {
            abort(404, 'Note not found.');
        }

        return $note;
    }

    /** ?filter[field]=value for the whitelisted fields, values cast like OpenGrid does. */
    private function filter(Builder $query, array $filters): void
    {
        foreach ($filters as $field => $value) {
            $type = Note::FILTERABLE[$field] ?? null;
            if ($type === null || ! is_string($value)) {
                continue;
            }
            $query->where($field, match ($type) {
                'bool' => filter_var($value, FILTER_VALIDATE_BOOLEAN),
                'int' => (int) $value,
            });
        }
    }

    /** ?sort=field[:asc|desc][,…]; anything else answers 400 (the field goes into the query). */
    private function sort(Builder $query, string $spec): void
    {
        if ($spec === '') {
            return;
        }
        foreach (explode(',', $spec) as $part) {
            [$field, $dir] = array_pad(explode(':', trim($part), 2), 2, 'asc');
            if (! in_array($field, Note::SORTABLE, true)) {
                abort(400, "invalid sort field `{$field}`");
            }
            $dir = strtolower($dir);
            if ($dir !== 'asc' && $dir !== 'desc') {
                abort(400, "invalid sort direction `{$dir}`");
            }
            $query->orderBy($field, $dir);
        }
    }
}
