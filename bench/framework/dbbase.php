<?php
// Database baseline for BM2 (BENCHMARKS §0.4): the SQL statements Laravel
// runs per request of a BM2 endpoint (bench/framework/querylog.py), sent
// without the framework through PDO pgsql with Laravel's PDO options to the
// database of the benchmark target. Each client is a forked process with its
// own connection.
//   php dbbase.php <endpoint[+touch]> <clients> <warmup s> <duration s> <persistent|connect>
//   persistent: one connection per client for the whole run (Octane)
//   connect:    a new connection for every request, with Laravel's
//               `set search_path to "public"` (FPM)
//   +touch:     Sanctum's token update on every request (it happens at most
//               once a second per token in the benchmark)
// Connection from the environment: PGHOST (the socket directory), PGPORT,
// PGDATABASE, PGUSER, PGPASSWORD. Prints one JSON line: ops, ops/s, p50, p99.

[$_, $endpoint, $clients, $warmup, $duration, $mode] = $argv + [null, 'show', 1, 3, 10, 'persistent'];
$touch = str_ends_with($endpoint, '+touch');
$endpoint = str_replace('+touch', '', $endpoint);
$clients = (int) $clients;
$dsn = sprintf('pgsql:host=%s;port=%s;dbname=%s', getenv('PGHOST'), getenv('PGPORT') ?: '5432', getenv('PGDATABASE'));
// Illuminate\Database\Connectors\Connector::$options
$options = [
    PDO::ATTR_CASE => PDO::CASE_NATURAL,
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_ORACLE_NULLS => PDO::NULL_NATURAL,
    PDO::ATTR_STRINGIFY_FETCHES => false,
    PDO::ATTR_EMULATE_PREPARES => false,
];

function connect(string $dsn, array $options): PDO
{
    $pdo = new PDO($dsn, getenv('PGUSER') ?: null, getenv('PGPASSWORD') ?: null, $options);
    $pdo->prepare('set search_path to "public"')->execute();
    return $pdo;
}

function q(PDO $pdo, string $sql, array $args): array
{
    // Laravel: prepare, bind, execute, fetch; the statement is freed afterwards
    $st = $pdo->prepare($sql);
    $st->execute($args);
    $rows = $st->fetchAll(PDO::FETCH_OBJ);
    $st = null;
    return $rows;
}

$seed = connect($dsn, $options);
$token = q($seed, 'select * from "personal_access_tokens" order by "id" limit 1', [])[0];
$user = q($seed, 'select * from "users" limit 1', [])[0];
$note = q($seed, 'select * from "notes" order by "id" limit 1 offset 100', [])[0];
$seed = null;

function once(PDO $pdo, string $endpoint, bool $touch, object $token, object $user, object $note, int $k): void
{
    $now = date('Y-m-d H:i:s');
    if ($endpoint === 'login') {
        q($pdo, 'select * from "users" where "email" = ? limit 1', [$user->email]);
        q($pdo, 'insert into "personal_access_tokens" ("name", "token", "abilities", "tokenable_id", "tokenable_type", "updated_at", "created_at") values (?, ?, ?, ?, ?, ?, ?) returning "id"',
            [$token->name, hash('sha256', "dbbase-$k-" . getmypid()), $token->abilities, $token->tokenable_id, $token->tokenable_type, $now, $now]);
        return;
    }
    q($pdo, 'select * from "personal_access_tokens" where "token" = ? limit 1', [$token->token]);
    q($pdo, 'select * from "users" where "users"."id" = ? limit 1', [$token->tokenable_id]);
    if ($touch) {
        q($pdo, 'update "personal_access_tokens" set "last_used_at" = ?, "updated_at" = ? where "id" = ?', [$now, $now, $token->id]);
    }
    switch ($endpoint) {
        case 'list':
            q($pdo, 'select count(*) as "aggregate" from "notes" where "user" = ? and "done" = ? and "notes"."deleted_at" is null', [$user->id, 'false']);
            q($pdo, 'select * from "notes" where "user" = ? and "done" = ? and "notes"."deleted_at" is null order by "priority" desc limit 15 offset 0', [$user->id, 'false']);
            break;
        case 'show':
            q($pdo, 'select * from "notes" where "notes"."id" = ? and "notes"."deleted_at" is null limit 1', [$note->id]);
            break;
        case 'create':
            q($pdo, 'insert into "notes" ("done", "priority", "title", "body", "user", "id", "updated_at", "created_at") values (?, ?, ?, ?, ?, ?, ?, ?)',
                ['false', 2, 'Bench', 'created by dbbase', $user->id, sprintf('%08x-%04x-4%03x-a%03x-%012x', getmypid(), $k & 0xffff, $k & 0xfff, ($k >> 12) & 0xfff, $k), $now, $now]);
            break;
        case 'health':
            break;
        default:
            fwrite(STDERR, "unknown endpoint $endpoint\n");
            exit(2);
    }
}

$start = microtime(true);
$from = $start + (float) $warmup;
$end = $from + (float) $duration;
$dir = sys_get_temp_dir();
$pids = [];
for ($c = 0; $c < $clients; $c++) {
    $pid = pcntl_fork();
    if ($pid === 0) {
        $pdo = $mode === 'persistent' ? connect($dsn, $options) : null;
        $lat = [];
        $k = 0;
        while (($t0 = microtime(true)) < $end) {
            $k++;
            $p = $pdo ?? connect($dsn, $options);
            once($p, $endpoint, $touch, $token, $user, $note, $k);
            $p = null; // connect mode: the connection closes with the request, as in FPM
            if ($t0 >= $from) {
                $lat[] = (microtime(true) - $t0) * 1000.0;
            }
        }
        file_put_contents("$dir/dbbase-" . getmypid() . '.json', json_encode($lat));
        exit(0);
    }
    $pids[] = $pid;
}
$all = [];
foreach ($pids as $pid) {
    pcntl_waitpid($pid, $status);
    $f = "$dir/dbbase-$pid.json";
    $all = array_merge($all, json_decode((string) @file_get_contents($f), true) ?: []);
    @unlink($f);
}
sort($all);
$n = count($all);
$pct = fn (float $p): float => $n ? $all[min($n - 1, (int) ($n * $p))] : 0.0;
printf("{\"ops\": %d, \"ops_per_s\": %.1f, \"p50_ms\": %.3f, \"p99_ms\": %.3f}\n", $n, $n / (float) $duration, $pct(0.5), $pct(0.99));
