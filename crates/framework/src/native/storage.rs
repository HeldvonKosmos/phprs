//! MS1: `OpenGrid\Storage\Storage` (Laravel's `Storage` facade) on Loco's
//! storage. `Storage::disk('name')` gives an `OpenGrid\Storage\Disk`; every
//! disk method is also a static call on `Storage` for the default disk.
//! Paths are checked (`services::check_path`): relative, no `..`.

use std::path::Path;

use grd_vm::error::Throw;
use grd_vm::value::{PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{ctx, string_arg};
use crate::services::check_path;

pub const FACADE: &str = "OpenGrid\\Storage\\Storage";
pub const DISK: &str = "OpenGrid\\Storage\\Disk";

/// Payload of a `Disk` object: the disk's name.
#[derive(Default, Clone)]
pub struct DiskRef(pub String);

fn storage(vm: &mut Vm, name: &str) -> Result<std::sync::Arc<loco_rs::storage::Storage>, Throw> {
    ctx(vm).services.disk(name).ok_or_else(|| Throw::error(format!("storage disk `{name}` is not configured")))
}

fn path_arg(c: &Call, i: usize, what: &str) -> Result<std::path::PathBuf, Throw> {
    check_path(&string_arg(c, i, what)?).map_err(Throw::value_error)
}

/// One disk operation; `disk` is the disk name, the arguments are the call's.
fn op(vm: &mut Vm, disk: &str, name: &str, c: &Call) -> Result<Val, Throw> {
    let st = storage(vm, disk)?;
    let services = ctx(vm).services.clone();
    let what = format!("Storage::{name}");
    Ok(match name {
        "put" => {
            let p = path_arg(c, 0, &what)?;
            let contents = match c.args.get(1) {
                Some(Val::Str(s)) => s.as_bytes().to_vec(),
                Some(o) => return Err(Throw::type_error(format!("{what}(): Argument #2 ($contents) must be of type string, {} given", o.type_name()))),
                None => return Err(Throw::argument_count(format!("{what}(): Argument #2 ($contents) is required"))),
            };
            services.block_on(st.upload(&p, &bytes::Bytes::from(contents))).map_err(|e| Throw::error(format!("{what}: {e}")))?;
            Val::Bool(true)
        }
        "get" => {
            let p = path_arg(c, 0, &what)?;
            if !services.block_on(st.exists(&p)).unwrap_or(false) {
                return Ok(Val::Null);
            }
            let data: Vec<u8> = services.block_on(st.download(&p)).map_err(|e| Throw::error(format!("{what}: {e}")))?;
            Val::bytes(&data)
        }
        "exists" | "missing" => {
            let p = path_arg(c, 0, &what)?;
            let e = services.block_on(st.exists(&p)).map_err(|e| Throw::error(format!("{what}: {e}")))?;
            Val::Bool(if name == "exists" { e } else { !e })
        }
        "delete" => {
            let paths: Vec<String> = match c.arg(0) {
                Val::Array(a) => a.values().map(|v| v.to_php_string().map(|s| s.to_string_lossy())).collect::<Result<_, _>>()?,
                Val::Str(s) => vec![s.to_string_lossy()],
                o => return Err(Throw::type_error(format!("{what}(): Argument #1 ($paths) must be of type string|array, {} given", o.type_name()))),
            };
            // Laravel (Flysystem 3): a missing file counts as deleted; a
            // failed delete makes the result false, the other paths still go
            let mut all = true;
            for p in paths {
                let p = check_path(&p).map_err(Throw::value_error)?;
                if services.block_on(st.exists(&p)).unwrap_or(false) && services.block_on(st.delete(&p)).is_err() {
                    all = false;
                }
            }
            Val::Bool(all)
        }
        "copy" | "move" => {
            let (from, to) = (path_arg(c, 0, &what)?, path_arg(c, 1, &what)?);
            if !services.block_on(st.exists(&from)).unwrap_or(false) {
                return Ok(Val::Bool(false));
            }
            let r = if name == "copy" { services.block_on(st.copy(&from, &to)) } else { services.block_on(st.rename(&from, &to)) };
            r.map_err(|e| Throw::error(format!("{what}: {e}")))?;
            Val::Bool(true)
        }
        "size" => {
            let p = path_arg(c, 0, &what)?;
            let data: Vec<u8> = services.block_on(st.download(&p)).map_err(|e| Throw::error(format!("{what}: {e}")))?;
            Val::Int(data.len() as i64)
        }
        // files($dir = '') lists the files directly in $dir, allFiles($dir = '') recursively
        "files" | "allFiles" => {
            let dir = match c.args.first() {
                None | Some(Val::Null) => String::new(),
                Some(_) => string_arg(c, 0, &what)?,
            };
            if !dir.is_empty() {
                check_path(&dir).map_err(Throw::value_error)?;
            }
            let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.trim_end_matches('/')) };
            let entries = services.block_on(st.list(Path::new(&prefix), name == "allFiles")).map_err(|e| Throw::error(format!("{what}: {e}")))?;
            let mut files: Vec<String> = entries.into_iter().filter(|e| !e.is_dir).map(|e| e.path.trim_start_matches('/').to_string()).collect();
            files.sort();
            Val::array(PhpArray::from_values(files.into_iter().map(|f| Val::str(&f))))
        }
        _ => return Err(Throw::error(format!("Call to undefined method {FACADE}::{name}()"))),
    })
}


pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let mut disk = NativeClassDef::new(DISK).final_class().payload(|| Box::new(DiskRef::default())).trace(grd_vm::vm::gc::trace_nothing);
    let mut facade = NativeClassDef::new(FACADE)
        .final_class()
        .static_method("disk", |vm, c| {
            let name = match c.args.first() {
                None | Some(Val::Null) => ctx(vm).services.default_disk.clone(),
                Some(_) => string_arg(&c, 0, "Storage::disk")?,
            };
            storage(vm, &name)?;
            let class = vm.lookup_class(DISK.as_bytes()).ok_or_else(|| Throw::error("Disk class missing"))?;
            let h = vm.instantiate(class, Vec::new())?;
            *vm.native_payload_mut::<DiskRef>(h).ok_or_else(|| Throw::error("Disk payload missing"))? = DiskRef(name);
            Ok(Val::Object(h))
        });
    macro_rules! both {
        ($name:literal) => {
            disk = disk.method($name, |vm, c| {
                let name = vm.native_payload::<DiskRef>(c.this()?).map(|d| d.0.clone()).ok_or_else(|| Throw::error("Disk payload missing"))?;
                op(vm, &name, $name, &c)
            });
            facade = facade.static_method($name, |vm, c| {
                let name = ctx(vm).services.default_disk.clone();
                op(vm, &name, $name, &c)
            });
        };
    }
    both!("put");
    both!("get");
    both!("exists");
    both!("missing");
    both!("delete");
    both!("copy");
    both!("move");
    both!("size");
    both!("files");
    both!("allFiles");
    vm.register_class(disk)?;
    vm.register_class(facade)?;
    Ok(())
}
