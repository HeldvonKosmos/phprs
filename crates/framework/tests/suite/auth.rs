//! AU1: token authentication like Laravel Sanctum — bcrypt passwords, 256-bit
//! bearer tokens stored as SHA-256, middleware `auth` (401 JSON), logout
//! revokes the token, the same 401 for unknown email and wrong password.
//! AU2: login throttle (429 after five failures per email and IP) and the
//! `throttle:N,M` middleware.

use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::native::database::schema::kit::SyncOptions;
use opengrid_framework::native::database::schema::ops;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::scaffold::new_project;
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

const USER: &str = r#"<?grd
namespace Users\Models;

use OpenGrid\Database\Model;

final class User extends Model
{
    protected static string $table = 'users';
    protected array $fillable = ['name', 'email', 'password'];
    protected array $hidden = ['password'];
}
"#;

const CONTROLLER: &str = r#"<?grd
namespace Users\Controllers;

use OpenGrid\Auth\Auth;
use OpenGrid\Http\Response;
use Users\Models\User;

final class AuthController
{
    public function register(): Response
    {
        $data = request()->validate(['name' => 'required|string', 'email' => 'required|email|unique:users,email', 'password' => 'required|string|min:8']);
        $user = User::create(['name' => $data['name'], 'email' => $data['email'], 'password' => password_hash($data['password'], PASSWORD_BCRYPT)]);
        return Response::json($user, 201);
    }

    public function login(): Response
    {
        $data = request()->validate(['email' => 'required|string', 'password' => 'required|string']);
        $user = Auth::attempt($data['email'], $data['password']);
        if ($user === null) {
            abort(401, 'Invalid credentials.');
        }
        return Response::json(['token' => Auth::createToken($user), 'user' => $user]);
    }

    public function me(): Response
    {
        return Response::json(Auth::user());
    }

    public function ping(): Response
    {
        return Response::json(['pong' => true]);
    }

    public function logout(): Response
    {
        Auth::revokeCurrentToken();
        return Response::noContent();
    }
}
"#;

const SCHEMA: &str = "DEFINE TABLE users SCHEMAFULL PERMISSIONS NONE;\nDEFINE FIELD name ON TABLE users TYPE string;\nDEFINE FIELD email ON TABLE users TYPE string;\nDEFINE FIELD password ON TABLE users TYPE string;\nDEFINE FIELD created_at ON TABLE users TYPE option<datetime>;\nDEFINE FIELD updated_at ON TABLE users TYPE option<datetime>;\nDEFINE INDEX users_email ON TABLE users FIELDS email UNIQUE;\n";

const ROUTES: &str = r#"<?grd
use OpenGrid\Routing\Route;
use Users\Controllers\AuthController;

Route::post('/api/register', [AuthController::class, 'register']);
Route::post('/api/login', [AuthController::class, 'login']);
Route::get('/api/ping', [AuthController::class, 'ping'])->middleware('throttle:3,1');
Route::group(['middleware' => ['auth']], function () {
    Route::get('/api/me', [AuthController::class, 'me']);
    Route::post('/api/logout', [AuthController::class, 'logout']);
});
"#;

fn send(pool: &Pool, mut req: HttpRequest, token: Option<&str>) -> HttpResponse {
    if let Some(t) = token {
        req.headers.push(("authorization".into(), format!("Bearer {t}")));
    }
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

fn json(r: &HttpResponse) -> serde_json::Value {
    serde_json::from_str(&r.body_str()).unwrap_or_else(|e| panic!("{e}: {}", r.body_str()))
}

/// A project with users, auth routes and its schema pushed; `auth` settings
/// are merged into the defaults.
fn auth_pool(tag: &str, auth: serde_json::Value) -> (std::path::PathBuf, Arc<Pool>) {
    let root = std::env::temp_dir().join(format!("opengrid-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Auth")).unwrap();
    for (path, text) in [("modules/users/app/Models/User.grd", USER), ("modules/users/app/Controllers/AuthController.grd", CONTROLLER), ("modules/users/routes/web.grd", ROUTES), ("modules/users/database/schema/schema.surql", SCHEMA)] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let mut auth_settings = serde_json::json!({ "model": "Users\\Models\\User" });
    if let (Some(a), Some(extra)) = (auth_settings.as_object_mut(), auth.as_object()) {
        a.extend(extra.clone());
    }
    let opts = PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", tag)).settings(serde_json::json!({ "app": { "name": "Auth" }, "auth": auth_settings }));
    let pool = Arc::new(Pool::start(opts).expect("project loads"));
    let db = pool.db().unwrap();
    db.block_on(ops::push(db.client(), &root, SyncOptions::default())).expect("schema");
    (root, pool)
}

fn last_used(pool: &Pool) -> String {
    pool.run_sync(|vm| {
        vm.run_file("q.grd", b"<?grd echo json_encode(OpenGrid\\Database\\DB::select('SELECT VALUE last_used_at FROM personal_access_tokens'));").unwrap();
        let out = String::from_utf8_lossy(&vm.output).into_owned();
        vm.reset_request();
        out
    })
    .unwrap()
}

fn login_token(pool: &Pool) -> String {
    let j = "application/json";
    assert_eq!(send(pool, HttpRequest::post("/api/register", j, br#"{"name":"Ada","email":"ada@example.com","password":"secret123"}"#), None).status, 201);
    let r = send(pool, HttpRequest::post("/api/login", j, br#"{"email":"ada@example.com","password":"secret123"}"#), None);
    json(&r)["token"].as_str().unwrap().to_string()
}

/// DBP1: the use of a token is written at most every `auth.token_touch_seconds`
/// (default 60), so read-only requests write nothing; 0 writes every time.
#[test]
fn token_use_is_noted_at_most_every_interval() {
    let (root, pool) = auth_pool("auth-touch", serde_json::json!({}));
    let token = login_token(&pool);
    assert_eq!(last_used(&pool), "[null]", "not used yet");
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&token)).status, 200);
    let first = last_used(&pool);
    assert!(first.starts_with("[\"20"), "first use noted: {first}");
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&token)).status, 200);
    assert_eq!(last_used(&pool), first, "within the interval nothing is written");
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);

    let (root, pool) = auth_pool("auth-touch0", serde_json::json!({ "token_touch_seconds": 0 }));
    let token = login_token(&pool);
    send(&pool, HttpRequest::get("/api/me"), Some(&token));
    let first = last_used(&pool);
    std::thread::sleep(std::time::Duration::from_millis(5));
    send(&pool, HttpRequest::get("/api/me"), Some(&token));
    let second = last_used(&pool);
    assert!(first.starts_with("[\"20") && second != first, "0 = every request: {first} {second}");
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn token_authentication() {
    let (root, pool) = auth_pool("auth", serde_json::json!({}));

    let j = "application/json";
    let r = send(&pool, HttpRequest::post("/api/register", j, br#"{"name":"Ada","email":"ada@example.com","password":"secret123"}"#), None);
    assert_eq!(r.status, 201, "{}", r.body_str());
    let user = json(&r);
    assert_eq!(user["email"], "ada@example.com");
    assert!(user.get("password").is_none(), "password is hidden");
    let r = send(&pool, HttpRequest::post("/api/register", j, br#"{"name":"Ada","email":"ada@example.com","password":"secret123"}"#), None);
    assert_eq!(r.status, 422);
    assert_eq!(json(&r)["errors"]["email"][0], "The email has already been taken.");

    // the same 401 for an unknown email and a wrong password
    let unknown = send(&pool, HttpRequest::post("/api/login", j, br#"{"email":"nobody@example.com","password":"secret123"}"#), None);
    let wrong = send(&pool, HttpRequest::post("/api/login", j, br#"{"email":"ada@example.com","password":"wrong-pass"}"#), None);
    assert_eq!((unknown.status, wrong.status), (401, 401));
    assert_eq!(unknown.body_str(), wrong.body_str());

    let r = send(&pool, HttpRequest::post("/api/login", j, br#"{"email":"ada@example.com","password":"secret123"}"#), None);
    assert_eq!(r.status, 200, "{}", r.body_str());
    let token = json(&r)["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 64, "256 bits as hex");

    // only the hash is stored
    let stored: String = pool.run_sync(|vm| {
        vm.run_file("q.grd", b"<?grd echo json_encode(OpenGrid\\Database\\DB::select('SELECT token_hash FROM personal_access_tokens'));").unwrap();
        String::from_utf8_lossy(&vm.output).into_owned()
    }).unwrap();
    assert!(!stored.contains(&token) && stored.contains("token_hash"), "{stored}");

    assert_eq!(send(&pool, HttpRequest::get("/api/me"), None).status, 401);
    assert_eq!(json(&send(&pool, HttpRequest::get("/api/me"), Some("deadbeef")))["message"], "Unauthenticated.");
    let r = send(&pool, HttpRequest::get("/api/me"), Some(&token));
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert_eq!(json(&r)["name"], "Ada");

    let r = send(&pool, HttpRequest::post("/api/logout", j, b""), Some(&token));
    assert_eq!(r.status, 204, "{}", r.body_str());
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&token)).status, 401, "token revoked");

    // AU2: the earlier success cleared the counter; five failures, then 429
    // even with the right password, with Retry-After; other emails unaffected
    let login = |body: &[u8]| send(&pool, HttpRequest::post("/api/login", j, body), None);
    for i in 0..5 {
        assert_eq!(login(br#"{"email":"ada@example.com","password":"wrong-pass"}"#).status, 401, "failure {}", i + 1);
    }
    let r = login(br#"{"email":"ADA@example.com","password":"secret123"}"#);
    assert_eq!(r.status, 429, "{}", r.body_str());
    let retry: i64 = r.header("retry-after").expect("Retry-After").parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    assert!(json(&r)["message"].as_str().unwrap().starts_with("Too many login attempts."), "{}", r.body_str());
    assert_eq!(login(br#"{"email":"nobody@example.com","password":"x"}"#).status, 401, "separate counter");

    // throttle:3,1 on /api/ping
    for _ in 0..3 {
        let r = send(&pool, HttpRequest::get("/api/ping"), None);
        assert_eq!(r.status, 200, "{}", r.body_str());
    }
    let r = send(&pool, HttpRequest::get("/api/ping"), None);
    assert_eq!(r.status, 429, "{}", r.body_str());
    assert_eq!((r.header("x-ratelimit-limit"), r.header("x-ratelimit-remaining")), (Some("3"), Some("0")));
    assert!(r.header("retry-after").is_some());
    let _ = std::fs::remove_dir_all(&root);
}

fn script(pool: &Pool, src: String) -> String {
    pool.run_sync(move |vm| {
        let r = vm.run_file("s.grd", src.as_bytes());
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("ERR {e}"));
        }
        vm.reset_request();
        out
    })
    .unwrap()
}

/// TEST2.7: a user may hold several tokens; logging out revokes only the
/// token of that request.
#[test]
fn several_tokens_per_user_and_logout_revokes_one() {
    let (root, pool) = auth_pool("auth-tokens", serde_json::json!({}));
    let first = login_token(&pool);
    let r = send(&pool, HttpRequest::post("/api/login", "application/json", br#"{"email":"ada@example.com","password":"secret123"}"#), None);
    let second = json(&r)["token"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    for t in [&first, &second] {
        assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(t)).status, 200);
    }
    assert_eq!(send(&pool, HttpRequest::post("/api/logout", "application/json", b""), Some(&first)).status, 204);
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&first)).status, 401, "revoked");
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&second)).status, 200, "the other token still works");
    assert_eq!(script(&pool, "<?grd echo OpenGrid\\Database\\DB::table('personal_access_tokens')->count();".into()), "1");
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}

/// TEST2.7: token expiry as in Sanctum: `auth.token_expiration` minutes after
/// creation, or the token's own `expires_at`.
#[test]
fn tokens_expire() {
    let (root, pool) = auth_pool("auth-expiry", serde_json::json!({ "token_expiration": 60 }));
    let token = login_token(&pool);
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&token)).status, 200, "fresh token");
    // 59 minutes old: valid; 61 minutes old: expired
    script(&pool, "<?grd OpenGrid\\Database\\DB::statement('UPDATE personal_access_tokens SET created_at = time::now() - 59m');".into());
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&token)).status, 200, "59 minutes old");
    script(&pool, "<?grd OpenGrid\\Database\\DB::statement('UPDATE personal_access_tokens SET created_at = time::now() - 61m');".into());
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&token)).status, 401, "61 minutes old");
    // expires_at of the token itself
    let make = |when: &str| {
        script(&pool, format!("<?grd use OpenGrid\\Auth\\Auth; use Users\\Models\\User; echo Auth::createToken(User::query()->first(), 'x', new DateTimeImmutable('{when}'));"))
    };
    let past = make("-1 minute");
    let future = make("+1 hour");
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&past)).status, 401, "past expires_at");
    assert_eq!(send(&pool, HttpRequest::get("/api/me"), Some(&future)).status, 200, "future expires_at");
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}

/// TOK2: noting the token's use never fails the request: many concurrent
/// requests with one token (each writes `last_used_at` with touch 0) all
/// answer 200, and even when the write is refused every time the request
/// is answered normally.
#[test]
fn noting_the_token_use_never_fails_a_request() {
    let (root, pool) = auth_pool("auth-tok2", serde_json::json!({ "token_touch_seconds": 0 }));
    let token = login_token(&pool);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (pool, token) = (pool.clone(), token.clone());
            std::thread::spawn(move || (0..25).map(|_| send(&pool, HttpRequest::get("/api/me"), Some(&token)).status).filter(|s| *s != 200).count())
        })
        .collect();
    let failed: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
    assert_eq!(failed, 0, "requests failed while noting the token use");
    // the database refuses every write of last_used_at: still 200
    let refused = script(&pool, "<?grd OpenGrid\\Database\\DB::statement('DEFINE FIELD OVERWRITE last_used_at ON personal_access_tokens TYPE option<datetime> ASSERT $value = NONE');".into());
    assert!(!refused.contains("ERR"), "{refused}");
    for _ in 0..3 {
        let r = send(&pool, HttpRequest::get("/api/me"), Some(&token));
        assert_eq!(r.status, 200, "{}", r.body_str());
        assert_eq!(json(&r)["email"], "ada@example.com");
    }
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}
