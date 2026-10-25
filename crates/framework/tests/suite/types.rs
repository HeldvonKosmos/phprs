//! SurrealDB types as native classes (DECISIONS 2026-10-24 "SurrealDB-Typen
//! nativ"): round trip of every type through a model against `mem://`, and
//! against a real server when `OPENGRID_TEST_DB_URL` is set.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::native::database::schema::kit::SyncOptions;
use opengrid_framework::native::database::schema::ops;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::{Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(cfg: DbConfig) -> Arc<Pool> {
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(1).database(cfg)).expect("pool starts"));
    let db = pool.db().expect("database");
    db.block_on(ops::push(db.client(), &root(), SyncOptions::default())).expect("schema push");
    pool
}

fn run(pool: &Pool, src: &'static str) -> String {
    pool.run_sync(move |vm| {
        let r = vm.run_file("t.grd", src.as_bytes());
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("ERR {e}"));
        }
        vm.reset_request();
        out
    })
    .expect("worker answers")
}

const ROUND_TRIP: &str = r#"<?grd
use OpenGrid\Database\Bytes;
use OpenGrid\Database\Decimal;
use OpenGrid\Database\Duration;
use OpenGrid\Database\Geometry;
use OpenGrid\Database\RecordId;
use OpenGrid\Database\Uuid;
use Tasks\Models\Measurement;
use Tasks\Models\Task;

$task = Task::create(['title' => 'typed']);
$m = Measurement::create([
    'label' => 'first',
    'amount' => Decimal::make('12.345'),
    'took' => Duration::make('1h30m'),
    'tag' => Uuid::parse('0193e0c2-7b1a-7cde-8f00-123456789abc'),
    'blob' => Bytes::make("\x00\x01binary"),
    'where' => Geometry::point(13.4, 52.5),
    'at' => new DateTimeImmutable('2026-10-24 12:30:00', new DateTimeZone('Europe/Berlin')),
    'task_id' => $task->id,
    'note' => null,
    'count' => 3,
    'ratio' => 0.25,
]);
$r = Measurement::find($m->id);
echo get_class($r->amount), ' ', $r->amount, ' ', $r->amount == Decimal::make('12.345') ? 'eq' : 'ne', "\n";
echo get_class($r->took), ' ', $r->took, ' ', $r->took->totalSeconds(), "\n";
echo get_class($r->tag), ' ', $r->tag, "\n";
echo get_class($r->blob), ' ', $r->blob->length(), ' ', $r->blob->toBase64(), "\n";
echo get_class($r->where), ' ', $r->where->type(), ' ', json_encode($r->where->coordinates()), "\n";
echo get_class($r->at), ' ', $r->at->format('Y-m-d H:i:s T'), ' ', $r->at->getTimestamp(), "\n";
echo get_class($r->task_id), ' ', $r->task_id == $task->id ? 'same' : 'diff', "\n";
echo isset($r->note) ? 'note' : 'no-note', ' ', $r->count, ' ', $r->ratio, ' ', get_debug_type($r->count), ' ', get_debug_type($r->ratio), "\n";
echo json_encode(['amount' => $r->amount, 'took' => $r->took, 'tag' => $r->tag, 'blob' => $r->blob, 'where' => $r->where, 'at' => $r->at, 'task' => $r->task_id]), "\n";

// arithmetic through the native handlers
$sum = $r->amount + Decimal::make('0.655');
echo get_class($sum), ' ', $sum, ' ', $r->amount * 2, ' ', 10 - $r->amount, ' ', $r->amount / 4, "\n";
$longer = $r->took + Duration::make('30m');
echo get_class($longer), ' ', $longer, ' ', $r->took * 2, ' ', $r->took > Duration::make('1h') ? 'gt' : 'le', "\n";
$later = $r->at + Duration::make('1d');
echo get_class($later), ' ', $later->format('Y-m-d H:i'), ' ', $later > $r->at ? 'later' : 'not', "\n";
try { $x = $r->amount + 'abc'; echo "no error\n"; } catch (TypeError $e) { echo "TypeError\n"; }

// null is NONE: the field is absent, whereNull finds it, filters follow the schema types
echo Measurement::query()->whereNull('note')->count(), ' ', Measurement::query()->whereNotNull('note')->count(), "\n";
echo Measurement::query()->filter(['count' => '3'])->count(), ' ', Task::query()->filter(['done' => 'false'])->count(), ' ', Measurement::query()->filter(['task_id' => (string) $task->id])->count(), "\n";
$r->note = 'now set';
$r->save();
echo Measurement::find($m->id)->note, ' ', Measurement::query()->whereNull('note')->count(), "\n";
$r->note = null;
$r->save();
echo Measurement::query()->whereNull('note')->count(), "\n";
"#;

const EXPECTED: &str = "OpenGrid\\Database\\Decimal 12.345 eq\n\
OpenGrid\\Database\\Duration 1h30m 5400\n\
OpenGrid\\Database\\Uuid 0193e0c2-7b1a-7cde-8f00-123456789abc\n\
OpenGrid\\Database\\Bytes 8 AAFiaW5hcnk=\n\
OpenGrid\\Database\\Geometry Point [13.4,52.5]\n\
DateTimeImmutable 2026-10-24 10:30:00 UTC 1792837800\n\
OpenGrid\\Database\\RecordId same\n\
no-note 3 0.25 int float\n\
{\"amount\":\"12.345\",\"took\":\"1h30m\",\"tag\":\"0193e0c2-7b1a-7cde-8f00-123456789abc\",\"blob\":\"AAFiaW5hcnk=\",\"where\":{\"type\":\"Point\",\"coordinates\":[13.4,52.5]},\"at\":\"2026-10-24T10:30:00+00:00\",\"task\":\"TASKID\"}\n\
OpenGrid\\Database\\Decimal 13 24.69 -2.345 3.08625\n\
OpenGrid\\Database\\Duration 2h 3h gt\n\
DateTimeImmutable 2026-10-25 10:30 later\n\
TypeError\n\
1 0\n\
1 1 1\n\
now set 0\n\
1\n";

fn check(out: &str) {
    // the record id is random; compare with it masked
    let masked = regex_mask(out);
    assert_eq!(masked, EXPECTED, "full output:\n{out}");
}

fn regex_mask(out: &str) -> String {
    let re = regex::Regex::new(r#""task":"tasks:[^"]+""#).unwrap();
    re.replace(out, r#""task":"TASKID""#).into_owned()
}

#[test]
fn round_trip_in_memory() {
    let pool = pool(DbConfig::mem("test", "types"));
    check(&run(&pool, ROUND_TRIP));
}

/// Real server (B2c): a throw-away `surreal` server, or OPENGRID_TEST_DB_URL.
#[test]
fn round_trip_real_server() {
    let Some(server) = crate::server::TestServer::start() else { return };
    let pool = pool(server.config("types"));
    check(&run(&pool, ROUND_TRIP));
    run(&pool, "<?grd\nuse OpenGrid\\Database\\DB;\nDB::statement('REMOVE TABLE IF EXISTS measurements'); DB::statement('REMOVE TABLE IF EXISTS tasks'); DB::statement('REMOVE TABLE IF EXISTS comments');");
}

/// B2a: a loaded model keeps the raw SDK fields and converts a field only when
/// it is read; `toArray()` converts the rest; writing a field drops its raw
/// value; `clone` keeps the whole payload.
#[test]
fn lazy_rows() {
    use grd_vm::value::{ObjHandle, Val};
    use grd_vm::vm::Vm;
    use opengrid_framework::native::database::model::ModelData;

    let pool = pool(DbConfig::mem("test", "lazy"));
    let seed = run(&pool, r#"<?grd
use OpenGrid\Database\{Bytes, Decimal, Duration, Geometry, Uuid};
use Tasks\Models\{Measurement, Task};
$t = Task::create(['title' => 'lazy']);
Measurement::create(['label' => 'm', 'amount' => Decimal::make('1.5'), 'took' => Duration::make('1s'), 'tag' => Uuid::v7(),
    'blob' => Bytes::make('x'), 'where' => Geometry::point(1, 2), 'at' => new DateTimeImmutable('2026-01-01'), 'task_id' => $t->id, 'count' => 2]);
$m = Measurement::query()->first();
$c = clone $m;
echo get_class($c->amount), ' ', $c->amount, ' ', $c->label, ' ', $c->id == $m->id ? 'same-id' : 'other-id';
"#);
    assert_eq!(seed, "OpenGrid\\Database\\Decimal 1.5 m same-id", "clone keeps the payload");

    fn state(vm: &Vm, h: ObjHandle) -> (usize, Vec<String>) {
        let d = vm.native_payload::<ModelData>(h).expect("model payload");
        (d.pending.len(), d.attributes.iter().filter(|(_, v)| !v.is_null()).map(|(k, _)| k.clone()).collect())
    }
    let lines = pool
        .run_sync(|vm| {
            let mut out = Vec::new();
            let first = |vm: &mut Vm| match vm.run_file("q.grd", b"<?grd\nreturn Tasks\\Models\\Measurement::query()->first();") {
                Ok(Val::Object(h)) => h,
                other => panic!("model expected: {other:?}"),
            };
            let h = first(vm);
            let (p0, c0) = state(vm, h);
            out.push(format!("loaded: pending>0={} converted={c0:?}", p0 > 0));
            let v = vm.get_property(h, "amount").expect("read amount");
            assert!(matches!(v, Val::Object(_)), "amount is a Decimal object");
            let (p1, c1) = state(vm, h);
            out.push(format!("read amount: pending-1={} converted={c1:?}", p1 + 1 == p0));
            vm.call_method_sync(h, b"toArray", Vec::new()).expect("toArray");
            let (p2, _) = state(vm, h);
            out.push(format!("toArray: pending={p2}"));

            let h = first(vm);
            vm.set_property(h, "label", Val::str("new")).expect("write label");
            let d = vm.native_payload::<ModelData>(h).expect("payload");
            out.push(format!("write label: raw-dropped={} dirty={:?}", !d.pending.contains_key("label"), d.dirty));
            vm.reset_request();
            out
        })
        .expect("worker answers");
    assert_eq!(
        lines,
        vec![
            "loaded: pending>0=true converted=[]".to_string(),
            "read amount: pending-1=true converted=[\"amount\"]".to_string(),
            "toArray: pending=0".to_string(),
            "write label: raw-dropped=true dirty=[\"label\"]".to_string(),
        ]
    );
}
