//! AU1: token authentication like Laravel Sanctum — bcrypt passwords, 256-bit
//! bearer tokens stored as SHA-256, middleware `auth` (401 JSON), logout
//! revokes the token, the same 401 for unknown email and wrong password.

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

#[test]
fn token_authentication() {
    let root = std::env::temp_dir().join(format!("opengrid-auth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Auth")).unwrap();
    for (path, text) in [("modules/users/app/Models/User.grd", USER), ("modules/users/app/Controllers/AuthController.grd", CONTROLLER), ("modules/users/routes/web.grd", ROUTES), ("modules/users/database/schema/schema.surql", SCHEMA)] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let opts = PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", "auth")).settings(serde_json::json!({ "app": { "name": "Auth" }, "auth": { "model": "Users\\Models\\User" } }));
    let pool = Arc::new(Pool::start(opts).expect("project loads"));
    let db = pool.db().unwrap();
    db.block_on(ops::push(db.client(), &root, SyncOptions::default())).expect("schema");

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
    let _ = std::fs::remove_dir_all(&root);
}
