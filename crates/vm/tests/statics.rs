//! DIALECT §5.4: static properties outlive the request, objects do not.
//! Writing an object (or an array holding one) to a static property is a
//! runtime error; enum cases are engine singletons and allowed.

use grd_vm::vm::Vm;

fn run(vm: &mut Vm, src: &str) -> String {
    vm.output.clear();
    let r = vm.run_file("t.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("ERR {e}"));
    }
    out
}

#[test]
fn objects_in_statics_are_rejected_enums_allowed() {
    let mut vm = Vm::with_prelude().expect("prelude");
    let out = run(
        &mut vm,
        r#"<?grd
enum Status: string { case Active = 'a'; case Done = 'd'; }
class Foo {}
class Cfg {
    public static mixed $any = null;
    public static array $list = [];
    public static Status $status = Status::Active;
    const DEFAULT = Status::Done;
    public static function set(mixed $v): void { static::$any = $v; }
}
try { Cfg::$any = new Foo(); echo "stored?\n"; } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
try { Cfg::set(new Foo()); echo "stored?\n"; } catch (Error $e) { echo "via method rejected\n"; }
try { Cfg::$list = ['x' => ['deep' => new Foo()]]; echo "stored?\n"; } catch (Error $e) { echo "nested rejected\n"; }
try { Cfg::$list[] = new Foo(); echo "stored?\n"; } catch (Error $e) { echo "append rejected\n"; }
Cfg::$status = Status::Done;
Cfg::$list = ['s' => Status::Active, 'n' => 1];
Cfg::$any = 'text';
echo Cfg::$status->name, ' ', Cfg::$list['s']->value, ' ', Cfg::DEFAULT->name, ' ', count(Cfg::$list), "\n";
"#,
    );
    assert_eq!(
        out,
        "Error: Cannot store an object of class Foo in static property Cfg::$any: static properties outlive the request; only enum cases are allowed\n\
         via method rejected\nnested rejected\nappend rejected\n\
         Done a Done 2\n"
    );

    // next request: the enum cases in statics and constants are still the same singletons
    vm.reset_request();
    assert_eq!(vm.objects.len(), 0);
    let out = run(
        &mut vm,
        r#"<?grd
echo Cfg::$status->name, ' ', Cfg::$status == Status::Done ? 'same' : 'different', ' ', Cfg::DEFAULT == Status::Done ? 'same' : 'different', ' ', Cfg::$list['s']->name, ' ', Cfg::$any, "\n";
var_dump(Status::from('a') == Status::Active);
"#,
    );
    assert_eq!(out, "Done same same Active text\nbool(true)\n");
}
