<?php
// Database baseline of the framework core (THESIS1, BENCHMARKS §0.4 method
// for SQLite): the statements one request of a core endpoint runs, without
// the framework, through PDO sqlite with Laravel's PDO options and pragmas,
// one connection. The same statements as BenchToken and the bench
// controllers; rows fetched as objects, as Laravel's select returns them.
// The token's sha256 is passed in (hashing is request work, not database
// work).
//
//   php sqlbase.php <database file> <token hash> <endpoint> <warmup s> <duration s>
//   endpoint: health | list | show:<id> | create | hydrate:<n> | collect:<n>
//             | validate:<n> | json:<n>   (validate and json: token only)
// Prints one JSON line: ops, ops/s, p50 and p99 in ms.

ini_set('memory_limit', '-1'); // millions of latencies for the empty endpoint
[$_, $path, $hash, $endpoint, $warmup, $duration] = $argv;
$pdo = new PDO('sqlite:'.$path, null, null, [
    PDO::ATTR_CASE => PDO::CASE_NATURAL,
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_ORACLE_NULLS => PDO::NULL_NATURAL,
    PDO::ATTR_STRINGIFY_FETCHES => false,
    PDO::ATTR_EMULATE_PREPARES => false,
]);
$pdo->exec('PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;');

function q(PDO $pdo, string $sql, array $bind): array
{
    $st = $pdo->prepare($sql);
    foreach ($bind as $i => $v) {
        $st->bindValue($i + 1, $v, is_int($v) ? PDO::PARAM_INT : PDO::PARAM_STR);
    }
    $st->execute();

    return $st->fetchAll(PDO::FETCH_OBJ);
}

[$name, $arg] = array_pad(explode(':', $endpoint, 2), 2, '0');
$one = function () use ($pdo, $hash, $name, $arg): void {
    if ($name === 'health') {
        return;
    }
    $row = q($pdo, 'SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1', [$hash])[0];
    $now = time();
    if ($now - $row->last_used_at >= 1) {
        $st = $pdo->prepare('UPDATE tokens SET last_used_at = ? WHERE id = ?');
        $st->execute([$now, $row->token_id]);
    }
    $uid = $row->id;
    switch ($name) {
        case 'list':
            q($pdo, 'SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL', [$uid, 0]);
            q($pdo, 'SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?', [$uid, 0, 15, 0]);
            break;
        case 'show':
            q($pdo, 'SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1', [(int) $arg, $uid]);
            break;
        case 'create':
            $now = gmdate('Y-m-d H:i:s');
            q($pdo, 'INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id', [$uid, 'Bench', 'created by bm2', 2, $now, $now]);
            break;
        case 'hydrate':
        case 'collect':
            q($pdo, 'SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?', [$uid, (int) $arg]);
            break;
        case 'validate':
        case 'json':
            break;
        default:
            fwrite(STDERR, "unknown endpoint $name\n");
            exit(2);
    }
};

$end = microtime(true) + (float) $warmup;
while (microtime(true) < $end) {
    $one();
}
$lat = [];
$start = hrtime(true);
$stop = $start + (int) ((float) $duration * 1e9);
do {
    $t = hrtime(true);
    $one();
    $now = hrtime(true);
    $lat[] = $now - $t;
} while ($now < $stop);
sort($lat);
$n = count($lat);
echo json_encode(['ops' => $n, 'ops_s' => $n / (($now - $start) / 1e9), 'p50_ms' => $lat[intdiv($n, 2)] / 1e6, 'p99_ms' => $lat[min($n - 1, intdiv($n * 99, 100))] / 1e6]), "\n";
