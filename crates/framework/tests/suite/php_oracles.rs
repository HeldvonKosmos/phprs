//! TEST2.6: small framework helpers against PHP and Laravel themselves:
//! query strings (`parse_str` of the dialect and the request's own parser)
//! against PHP's `parse_str`, model inflection against Laravel's
//! `Str::singular`, task class names against `Str::studly`.

use std::path::Path;
use std::process::Command;

use grd_vm::value::{ArrayKey, Val};
use opengrid_framework::control::task_class;
use opengrid_framework::http::parse_query;
use opengrid_framework::native::database::inflect::singular;
use opengrid_framework::native::request::nested_array;

const QUERIES: &[&str] = &[
    "a=1&b=2",
    "filters[done]=1&tags[]=a&tags[]=b&sort=x",
    "a[b]=1&a[c][]=2&a[c][]=3",
    "x[]=1&x[5]=2&x[]=3",
    "a=1&a=2",
    "a[b][c]=1&a[b]=2",
    "a[b]=2&a[b][c]=1",
    "a[][x]=1&a[][x]=2",
    "a[=1&b]=2&c[d=3",
    "a.b=1&c+d=2&e%20f=3",
    "a[x.y]=1&a[x y]=2",
    "%5Bk%5D=1&k%5B0%5D=2",
    "a&b=&=c&&d=%41%42",
    "q=caf%C3%A9+au+lait&r=%2B%26%3D",
    "a[0]=x&a[1]=y&a[3]=z",
    "a[-1]=x&a[01]=y&a[1]=z",
];

fn php_ok() -> bool {
    Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success())
}

fn php(code: &str) -> String {
    let out = Command::new("php").arg("-r").arg(code).output().expect("run php");
    assert!(out.status.success(), "php failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf-8")
}

/// A PHP single-quoted string literal.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// JSON with every array as an object (keys in order), as PHP's
/// `json_encode($v, JSON_FORCE_OBJECT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE)`.
fn forced_json(v: &Val) -> String {
    match v {
        Val::Array(a) => {
            let items: Vec<String> = a
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        ArrayKey::Int(i) => i.to_string(),
                        ArrayKey::Str(s) => s.to_string_lossy(),
                    };
                    format!("{}:{}", serde_json::to_string(&key).unwrap(), forced_json(v))
                })
                .collect();
            format!("{{{}}}", items.join(","))
        }
        Val::Str(s) => serde_json::to_string(&s.to_string_lossy()).unwrap(),
        other => panic!("unexpected value {other:?}"),
    }
}

#[test]
fn query_strings_parse_like_php() {
    if !php_ok() {
        crate::oracle::missing(crate::oracle::Oracle::Php, "php not available: query parsing not compared with parse_str");
        return;
    }
    let list = QUERIES.iter().map(|q| quote(q)).collect::<Vec<_>>().join(", ");
    let want: Vec<String> = php(&format!("foreach ([{list}] as $q) {{ parse_str($q, $r); echo json_encode($r, JSON_FORCE_OBJECT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE), \"\\n\"; }}")).lines().map(String::from).collect();
    assert_eq!(want.len(), QUERIES.len());
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    let src = format!("<?grd\nforeach ([{list}] as $q) {{ echo json_encode(parse_str($q), JSON_FORCE_OBJECT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE), \"\\n\"; }}");
    vm.run_file("q.grd", src.as_bytes()).expect("parse_str script");
    let dialect: Vec<String> = String::from_utf8_lossy(&vm.output).lines().map(String::from).collect();
    let mut failures = Vec::new();
    for (i, q) in QUERIES.iter().enumerate() {
        let request = forced_json(&nested_array(&parse_query(q)));
        if dialect.get(i) != Some(&want[i]) {
            failures.push(format!("parse_str({q:?}): grd {:?}, php {}", dialect.get(i), want[i]));
        }
        if request != want[i] {
            failures.push(format!("request query {q:?}: grd {request}, php {}", want[i]));
        }
    }
    assert!(failures.is_empty(), "{} differ from PHP's parse_str:\n{}", failures.len(), failures.join("\n"));
}

fn laravel(code: &str) -> Option<String> {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/laravel-app");
    if !php_ok() || !app.join("vendor/autoload.php").exists() {
        crate::oracle::missing(crate::oracle::Oracle::LaravelVendor, "php or bench/laravel-app/vendor not available: Str helpers not compared with Laravel");
        return None;
    }
    Some(php(&format!("require {}; {code}", quote(&app.join("vendor/autoload.php").display().to_string()))))
}

const PLURALS: &[&str] = &[
    "comments", "entries", "boxes", "addresses", "people", "status", "hours", "categories", "children", "statuses", "news", "users", "notes",
    "personal_access_tokens", "failed_jobs", "analyses", "indices", "quizzes", "buses", "wolves", "mice", "data", "valves", "houses", "archives",
    "knives", "halves", "prices", "bases", "viruses", "aliases", "octopuses", "shelves", "lives", "campuses", "crises", "theses", "axes", "matrices",
    "vertices", "geese", "teeth", "feet", "heroes", "potatoes", "photos", "movies", "series", "species", "shoes", "horses", "cases", "phases",
    "responses", "databases", "processes", "taxes", "oxen", "sheep", "fish", "logs", "access_tokens", "sizes", "order_items", "user_addresses",
];

#[test]
fn table_names_singularize_like_laravel() {
    let list = PLURALS.iter().map(|w| quote(w)).collect::<Vec<_>>().join(", ");
    let Some(out) = laravel(&format!("foreach ([{list}] as $w) echo Illuminate\\Support\\Str::singular($w), \"\\n\";")) else { return };
    let want: Vec<&str> = out.lines().collect();
    let diff: Vec<String> = PLURALS.iter().zip(&want).filter(|(p, w)| singular(p) != **w).map(|(p, w)| format!("{p}: ours {}, Laravel {w}", singular(p))).collect();
    assert_eq!(want.len(), PLURALS.len());
    assert!(diff.is_empty(), "singular differs from Laravel's Str::singular:\n{}", diff.join("\n"));
}

const TASKS: &[&str] = &["notes:prune", "users:send-digest", "billing:close_month", "a:b-c_d", "reports:monthly-pdf-export", "x:already_Studly"];

#[test]
fn task_classes_are_laravels_studly_names() {
    let list = TASKS.iter().map(|t| quote(t)).collect::<Vec<_>>().join(", ");
    let code = format!("foreach ([{list}] as $t) {{ [$m, $n] = explode(':', $t); echo Illuminate\\Support\\Str::studly($m), '\\\\Tasks\\\\', Illuminate\\Support\\Str::studly($n), \"\\n\"; }}");
    let Some(out) = laravel(&code) else { return };
    let want: Vec<&str> = out.lines().collect();
    assert_eq!(want.len(), TASKS.len());
    let diff: Vec<String> = TASKS.iter().zip(&want).filter(|(t, w)| task_class(t).as_deref() != Some(**w)).map(|(t, w)| format!("{t}: ours {:?}, Laravel {w}", task_class(t))).collect();
    assert!(diff.is_empty(), "task classes differ from Laravel's Str::studly:\n{}", diff.join("\n"));
}
