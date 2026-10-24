//! I1: constructor injection for controllers and middleware — recursive,
//! interfaces bind to their one implementation, the request is injected,
//! an ambiguous interface is GRD022.

use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::scaffold::new_project;
use opengrid_framework::{HttpRequest, Pool, PoolOptions};

const FILES: &[(&str, &str)] = &[
    ("modules/shop/app/Services/Clock.grd", "<?grd\nnamespace Shop\\Services;\n\ninterface Clock\n{\n    public function now(): string;\n}\n"),
    ("modules/shop/app/Services/FixedClock.grd", "<?grd\nnamespace Shop\\Services;\n\nfinal class FixedClock implements Clock\n{\n    public function now(): string { return '12:00'; }\n}\n"),
    ("modules/shop/app/Services/Greeter.grd", "<?grd\nnamespace Shop\\Services;\n\nfinal class Greeter\n{\n    public function __construct(private Clock $clock, private string $greeting = 'Hallo') {}\n    public function greet(string $name): string { return $this->greeting . ' ' . $name . ' um ' . $this->clock->now(); }\n}\n"),
    ("modules/shop/app/Services/Store.grd", "<?grd\nnamespace Shop\\Services;\n\ninterface Store {}\n"),
    ("modules/shop/app/Services/FileStore.grd", "<?grd\nnamespace Shop\\Services;\n\nfinal class FileStore implements Store {}\n"),
    ("modules/shop/app/Services/MemoryStore.grd", "<?grd\nnamespace Shop\\Services;\n\nfinal class MemoryStore implements Store {}\n"),
    ("modules/shop/app/Http/Middleware/Stamp.grd", "<?grd\nnamespace Shop\\Http\\Middleware;\n\nuse OpenGrid\\Http\\Middleware;\nuse OpenGrid\\Http\\Request;\nuse OpenGrid\\Http\\Response;\nuse Shop\\Services\\Clock;\n\nfinal class Stamp extends Middleware\n{\n    public function __construct(private Clock $clock) {}\n    public function handle(Request $request, \\Closure $next): mixed\n    {\n        $response = $next($request);\n        if ($response instanceof Response) { $response->header('X-Clock', $this->clock->now()); }\n        return $response;\n    }\n}\n"),
    ("modules/shop/app/Controllers/HelloController.grd", "<?grd\nnamespace Shop\\Controllers;\n\nuse OpenGrid\\Http\\Request;\nuse OpenGrid\\Http\\Response;\nuse Shop\\Services\\Greeter;\n\nfinal class HelloController\n{\n    public function __construct(private Greeter $greeter, private Request $request) {}\n    public function show(): Response { return Response::json(['text' => $this->greeter->greet((string) $this->request->query('name', 'Welt'))]); }\n}\n"),
    ("modules/shop/app/Controllers/StoreController.grd", "<?grd\nnamespace Shop\\Controllers;\n\nuse Shop\\Services\\Store;\n\nfinal class StoreController\n{\n    public function __construct(private Store $store) {}\n    public function show(): array { return ['ok' => true]; }\n}\n"),
    ("modules/shop/routes/web.grd", "<?grd\nuse OpenGrid\\Routing\\Route;\nuse Shop\\Controllers\\HelloController;\nuse Shop\\Controllers\\StoreController;\nuse Shop\\Http\\Middleware\\Stamp;\n\nRoute::aliasMiddleware('stamp', Stamp::class);\nRoute::get('/hello', [HelloController::class, 'show'])->middleware('stamp');\nRoute::get('/store', [StoreController::class, 'show']);\n"),
];

#[test]
fn constructor_injection() {
    let root = std::env::temp_dir().join(format!("opengrid-inject-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Inject")).unwrap();
    for (path, text) in FILES {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(1)).expect("project loads"));
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/hello?name=Ada"))).unwrap();
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert_eq!(r.body_str(), r#"{"text":"Hallo Ada um 12:00"}"#);
    assert_eq!(r.header("x-clock"), Some("12:00"), "middleware got its dependency");
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/store"))).unwrap();
    assert_eq!(r.status, 500);
    assert!(r.body_str().contains("GRD022") && r.body_str().contains("Shop\\\\Services\\\\FileStore"), "{}", r.body_str());
    let _ = std::fs::remove_dir_all(&root);
}
