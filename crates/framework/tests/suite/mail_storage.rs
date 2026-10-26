//! MS1: Mail and Storage with Laravel's feel on Loco's mailer and storage.
//! Without SMTP settings mails are logged and kept in the outbox
//! (`Mail::sent()`); storage paths cannot leave the disk.

use std::sync::Arc;

use opengrid_framework::scaffold::new_project;
use opengrid_framework::{Pool, PoolOptions};

const MAILABLE: &str = r#"<?grd
namespace Users\Mail;

use OpenGrid\Mail\Mailable;

final class WelcomeMail extends Mailable
{
    public function __construct(private string $name) {}

    public function build(): void
    {
        $this->view('welcome', ['name' => $this->name, 'items' => ['a', 'b']]);
    }
}
"#;

fn project(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("opengrid-ms1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Ms1")).unwrap();
    let dir = root.join("modules/users/resources/mail/welcome");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("subject.t"), "Welcome, {{ name }}!\n").unwrap();
    std::fs::write(dir.join("html.t"), "<p>Hello {{ name }}</p>{% for i in items %}<i>{{ i }}</i>{% endfor %}").unwrap();
    std::fs::write(dir.join("text.t"), "Hello {{ name }}").unwrap();
    let p = root.join("modules/users/app/Mail/WelcomeMail.grd");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, MAILABLE).unwrap();
    root
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

#[test]
fn mail_through_templates_and_outbox() {
    let root = project("mail");
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(1).settings(serde_json::json!({ "mailer": { "from": "App <app@example.com>" } }))).expect("pool"));
    let out = run(
        &pool,
        r#"<?grd
use OpenGrid\Mail\Mail;
use OpenGrid\Mail\Mailable;
use Users\Mail\WelcomeMail;

Mail::to('ada@example.com')->cc(['b@example.com'])->send(new WelcomeMail('Ada <3'));
final class Plain extends Mailable {
    public function build(): void { $this->to('c@example.com')->subject('Plain')->text('just text')->from('other@example.com'); }
}
Mail::send(new Plain());
$sent = Mail::sent();
echo count($sent), "\n";
echo json_encode([$sent[0]['to'], $sent[0]['cc'], $sent[0]['from'], $sent[0]['subject'], $sent[0]['html'], $sent[0]['text']]), "\n";
echo json_encode([$sent[1]['to'], $sent[1]['from'], $sent[1]['subject'], $sent[1]['text']]), "\n";
try { Mail::send(new Plain() ); } catch (Error $e) { echo 'x'; }
final class NoView extends Mailable { public function build(): void { $this->to('d@example.com')->view('nope'); } }
try { Mail::send(new NoView()); } catch (Error $e) { echo $e->getMessage(), "\n"; }
final class NoOne extends Mailable { public function build(): void { $this->text('x'); } }
try { Mail::send(new NoOne()); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.first().copied(), Some("2"), "{out}");
    assert_eq!(lines.get(1).copied(), Some(r#"[["ada@example.com"],["b@example.com"],"App <app@example.com>","Welcome, Ada <3!","<p>Hello Ada &lt;3<\/p><i>a<\/i><i>b<\/i>","Hello Ada <3"]"#), "{out}");
    assert_eq!(lines.get(2).copied(), Some(r#"[["c@example.com"],"other@example.com","Plain","just text"]"#), "{out}");
    assert!(lines.get(3).is_some_and(|l| l.contains("mail view `nope` not found")), "{out}");
    assert_eq!(lines.get(4).copied(), Some("mail without recipient"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn smtp_failure_is_an_error_not_a_crash() {
    let root = project("smtp");
    // nothing listens on port 1
    let settings = serde_json::json!({ "mailer": { "smtp": { "host": "127.0.0.1", "port": 1 } } });
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(1).settings(settings)).expect("pool"));
    let out = run(&pool, "<?grd\nuse OpenGrid\\Mail\\Mail;\nuse Users\\Mail\\WelcomeMail;\ntry { Mail::to('a@example.com')->send(new WelcomeMail('A')); echo 'sent'; } catch (Error $e) { echo str_starts_with($e->getMessage(), 'mail could not be sent') ? 'error' : $e->getMessage(); }");
    assert_eq!(out, "error");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn storage_disks() {
    let root = project("storage");
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(1)).expect("pool"));
    let out = run(
        &pool,
        r#"<?grd
use OpenGrid\Storage\Storage;

echo json_encode([Storage::put('docs/a.txt', 'alpha'), Storage::exists('docs/a.txt'), Storage::missing('docs/b.txt'), Storage::get('docs/a.txt'), Storage::get('docs/none.txt'), Storage::size('docs/a.txt')]), "\n";
Storage::put('docs/sub/c.txt', 'gamma');
echo json_encode([Storage::copy('docs/a.txt', 'docs/b.txt'), Storage::move('docs/b.txt', 'docs/moved.txt'), Storage::exists('docs/b.txt'), Storage::get('docs/moved.txt')]), "\n";
echo json_encode([Storage::files('docs'), Storage::allFiles('docs')]), "\n";
echo json_encode([Storage::delete(['docs/a.txt', 'docs/moved.txt']), Storage::delete('docs/a.txt'), Storage::files('docs')]), "\n";
$mem = Storage::disk('memory');
$mem->put('x.bin', "\x00\x01");
echo json_encode([strlen($mem->get('x.bin')), Storage::exists('x.bin'), $mem->exists('x.bin')]), "\n";
foreach (['../escape.txt', '/etc/passwd', 'docs/../../x', 'a\\b'] as $bad) {
    try { Storage::put($bad, 'x'); echo 'accepted '; } catch (ValueError $e) { echo 'rejected '; }
}
echo "\n";
try { Storage::disk('s3'); } catch (Error $e) { echo $e->getMessage(), "\n"; }
"#,
    );
    let want = [
        r#"[true,true,true,"alpha",null,5]"#,
        r#"[true,true,false,"alpha"]"#,
        r#"[["docs\/a.txt","docs\/moved.txt"],["docs\/a.txt","docs\/moved.txt","docs\/sub\/c.txt"]]"#,
        // Laravel 13.38 local disk: deleting a missing file is true as well
        r#"[true,true,[]]"#,
        "[2,false,true]",
        "rejected rejected rejected rejected ",
        "storage disk `s3` is not configured",
    ];
    for (i, w) in want.iter().enumerate() {
        assert_eq!(out.lines().nth(i), Some(*w), "line {}:\n{out}", i + 1);
    }
    assert_eq!(std::fs::read_to_string(root.join("storage/app/docs/sub/c.txt")).unwrap(), "gamma", "the local disk is <project>/storage/app");
    assert!(!root.join("escape.txt").exists() && !root.join("storage/escape.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}
