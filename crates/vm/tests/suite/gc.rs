//! MEM1b: objects are collected inside a request; whatever is still reachable
//! (locals, arrays, properties, closures, generators, statics) survives, and
//! nothing is collected inside native callbacks.

fn run(src: &str, threshold: usize) -> (String, grd_vm::vm::gc::GcStats, usize) {
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    vm.set_gc_threshold(threshold);
    vm.object_limit = 0;
    let r = vm.run_file("gc.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("\nFAILED: {e}"));
    }
    (out, vm.gc_stats, vm.objects.len())
}

#[test]
fn short_lived_objects_are_collected() {
    let src = "<?grd
final class P { public function __construct(public int $a) {} }
function work(): int { $sum = 0; for ($i = 0; $i < 300000; $i++) { $p = new P($i); $sum += $p->a; } return $sum; }
echo work();";
    let (out, stats, live) = run(src, 10_000);
    assert_eq!(out, "44999850000");
    assert!(stats.runs > 0 && stats.freed > 250_000, "{stats:?}");
    assert!(live < 30_000, "live objects after the loop: {live}");
    assert_eq!(stats.opaque, 0);
}

#[test]
fn reachable_objects_survive() {
    let src = r#"<?grd
final class Node { public ?Node $next = null; public array $tags = []; public function __construct(public int $n) {} }
final class Registry { public static array $names = []; }
function gen(Node $n): Generator { yield $n; yield new Node(-1); }
$keep = [];
$head = null;
$closures = [];
for ($i = 0; $i < 50000; $i++) {
    $n = new Node($i);
    if ($i % 1000 === 0) {
        $keep[] = $n;
        $n->tags = ['self' => [$n], 'obj' => new Node($i + 1)];
        $node = $n;
        $closures[] = fn (): int => $node->n;
    }
    if ($i % 5000 === 0) { $n->next = $head; $head = $n; }
}
$g = gen(new Node(7));
for ($i = 0; $i < 20000; $i++) { $tmp = new Node($i); }
$sum = 0;
foreach ($keep as $k) { $sum += $k->n + $k->tags['obj']->n + $k->tags['self'][0]->n; }
$chain = 0;
for ($c = $head; $c !== null; $c = $c->next) { $chain++; }
$cl = 0;
foreach ($closures as $f) { $cl += $f(); }
echo $sum, ' ', $chain, ' ', $cl, ' ', $g->current()->n, ' ';
$g->next();
echo $g->current()->n;
"#;
    let (out, stats, _) = run(src, 1_000);
    // 50 kept nodes: n + (n + 1) + n, chain of 10, closures over the kept nodes
    let kept: i64 = (0..50).map(|k| k * 1000).sum();
    assert_eq!(out, format!("{} 10 {} 7 -1", 3 * kept + 50, kept), "{stats:?}");
    assert!(stats.runs > 0, "{stats:?}");
}

#[test]
fn no_collection_inside_native_callbacks() {
    // array_map keeps its results in Rust until it returns: objects created
    // by the callback must not be collected meanwhile
    let src = "<?grd
final class P { public function __construct(public int $a) {} }
$r = array_map(fn (int $i): P => new P($i), range(1, 30000));
$sum = 0;
foreach ($r as $p) { $sum += $p->a; }
echo $sum;";
    let (out, stats, _) = run(src, 100);
    assert_eq!(out, "450015000", "{stats:?}");
}

#[test]
fn tiny_threshold_and_cycles() {
    // threshold 1: the first collection at the first safepoint, then whenever
    // the live objects have doubled; self-referencing garbage goes too
    let src = "<?grd
$all = [];
for ($i = 0; $i < 2000; $i++) { $o = new stdClass(); $o->i = $i; $o->self = $o; if ($i % 2 === 0) { $all[] = $o; } }
$s = 0;
foreach ($all as $o) { $s += $o->self->i; }
echo $s, ' ', count($all);";
    let (out, stats, live) = run(src, 1);
    assert_eq!(out, "999000 1000");
    assert!(stats.runs > 5 && stats.freed > 500, "{stats:?}");
    assert!(live < 2000, "live: {live}");
}
