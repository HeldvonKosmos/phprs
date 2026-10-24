//! `Name::class` is a compile-time string like in PHP: resolved through the
//! current namespace, `use` imports and aliases; the class need not exist.
//! `self::`/`static::`/`parent::class` stay runtime lookups.

use grd_vm::vm::Vm;

fn run(src: &str) -> String {
    let mut vm = Vm::with_prelude().expect("prelude");
    let r = vm.run_file("t.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("ERR {e}"));
    }
    out
}

#[test]
fn class_name_constant_follows_namespace_and_imports() {
    let out = run(r#"<?grd
namespace App\Services;

use App\Models\User;
use App\Models\Order as O;
use Vendor\Lib\{Reader, Writer as W};
use function App\Helpers\format;

class Local {}
class Child extends Local {
    public function names(): string { return self::class . ',' . static::class . ',' . parent::class; }
}

echo User::class, "|", O::class, "|", Reader::class, "|", W::class, "|", Local::class, "|", Models\Thing::class, "|", \Global\Thing::class, "|", namespace\Deep\Thing::class, "\n";
echo (new Child())->names(), "\n";
echo \strlen(User::class), "\n";
"#);
    assert_eq!(
        out,
        "App\\Models\\User|App\\Models\\Order|Vendor\\Lib\\Reader|Vendor\\Lib\\Writer|App\\Services\\Local|App\\Services\\Models\\Thing|Global\\Thing|App\\Services\\Deep\\Thing\n\
         App\\Services\\Child,App\\Services\\Child,App\\Services\\Local\n\
         15\n"
    );
}

#[test]
fn class_name_constant_in_global_namespace() {
    let out = run(r#"<?grd
use Foo\Bar;
echo Bar::class, "|", Baz::class, "|", \Qux\Quux::class, "\n";
"#);
    assert_eq!(out, "Foo\\Bar|Baz|Qux\\Quux\n");
}
