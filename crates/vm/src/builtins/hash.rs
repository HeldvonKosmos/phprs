//! Hashing on the RustCrypto crates (maintained, already in the dependency
//! tree through SurrealDB): `md5`, `sha1`, `hash`, `hash_hmac`, `hash_equals`,
//! `hash_algos`, `hash_hmac_algos`. `crc32b` stays in `string.rs`.

use digest::Digest;
use hmac::{Hmac, Mac};
use subtle::ConstantTimeEq;

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

const ALGOS: [&str; 7] = ["md5", "sha1", "sha224", "sha256", "sha384", "sha512", "crc32b"];
const HMAC_ALGOS: [&str; 6] = ["md5", "sha1", "sha224", "sha256", "sha384", "sha512"];

/// Raw digest of `data`, or None for an unknown algorithm.
fn digest(algo: &str, data: &[u8]) -> Option<Vec<u8>> {
    Some(match algo {
        "md5" => md5::Md5::digest(data).to_vec(),
        "sha1" => sha1::Sha1::digest(data).to_vec(),
        "sha224" => sha2::Sha224::digest(data).to_vec(),
        "sha256" => sha2::Sha256::digest(data).to_vec(),
        "sha384" => sha2::Sha384::digest(data).to_vec(),
        "sha512" => sha2::Sha512::digest(data).to_vec(),
        "crc32b" => super::string::crc32(data).to_be_bytes().to_vec(),
        _ => return None,
    })
}

fn hmac_raw(algo: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    macro_rules! mac {
        ($d:ty) => {{
            let mut m = Hmac::<$d>::new_from_slice(key).expect("HMAC takes keys of any length");
            m.update(data);
            m.finalize().into_bytes().to_vec()
        }};
    }
    Some(match algo {
        "md5" => mac!(md5::Md5),
        "sha1" => mac!(sha1::Sha1),
        "sha224" => mac!(sha2::Sha224),
        "sha256" => mac!(sha2::Sha256),
        "sha384" => mac!(sha2::Sha384),
        "sha512" => mac!(sha2::Sha512),
        _ => return None,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex (default) or raw bytes (`$binary = true`), as PHP returns them.
fn output(bytes: Vec<u8>, binary: bool) -> Val {
    if binary { Val::Str(PhpString::from(bytes)) } else { Val::from(hex(&bytes)) }
}

pub fn register(vm: &mut Vm) {
    vm.register_native("md5", |_, a| Ok(output(digest("md5", args::string(a, 0, "md5")?.as_bytes()).expect("md5"), args::boolean(a, 1, false)?)));
    vm.register_native("sha1", |_, a| Ok(output(digest("sha1", args::string(a, 0, "sha1")?.as_bytes()).expect("sha1"), args::boolean(a, 1, false)?)));
    vm.register_native("hash", |_, a| {
        let algo = args::string(a, 0, "hash")?.to_string_lossy().to_lowercase();
        let data = args::string(a, 1, "hash")?;
        let bytes = digest(&algo, data.as_bytes()).ok_or_else(|| Throw::value_error(format!("hash(): Argument #1 ($algo) must be a valid hashing algorithm")))?;
        Ok(output(bytes, args::boolean(a, 2, false)?))
    });
    vm.register_native("hash_hmac", |_, a| {
        let algo = args::string(a, 0, "hash_hmac")?.to_string_lossy().to_lowercase();
        let data = args::string(a, 1, "hash_hmac")?;
        let key = args::string(a, 2, "hash_hmac")?;
        let bytes = hmac_raw(&algo, key.as_bytes(), data.as_bytes()).ok_or_else(|| Throw::value_error("hash_hmac(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm"))?;
        Ok(output(bytes, args::boolean(a, 3, false)?))
    });
    // constant time, so the comparison of secrets does not leak where they differ
    vm.register_native("hash_equals", |_, a| {
        let known = match a.first() {
            Some(Val::Str(s)) => s.clone(),
            Some(o) => return Err(Throw::type_error(format!("hash_equals(): Argument #1 ($known_string) must be of type string, {} given", o.type_name()))),
            None => return Err(Throw::argument_count("hash_equals() expects exactly 2 arguments, 0 given")),
        };
        let user = match a.get(1) {
            Some(Val::Str(s)) => s.clone(),
            Some(o) => return Err(Throw::type_error(format!("hash_equals(): Argument #2 ($user_string) must be of type string, {} given", o.type_name()))),
            None => return Err(Throw::argument_count("hash_equals() expects exactly 2 arguments, 1 given")),
        };
        Ok(Val::Bool(known.len() == user.len() && bool::from(known.as_bytes().ct_eq(user.as_bytes()))))
    });
    vm.register_native("hash_algos", |_, _| Ok(Val::array(PhpArray::from_values(ALGOS.map(Val::str)))));
    vm.register_native("hash_hmac_algos", |_, _| Ok(Val::array(PhpArray::from_values(HMAC_ALGOS.map(Val::str)))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(hex(&digest("sha256", b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&digest("md5", b"").unwrap()), "d41d8cd98f00b204e9800998ecf8427e");
        // RFC 4231 test case 2
        assert_eq!(hex(&hmac_raw("sha256", b"Jefe", b"what do ya want for nothing?").unwrap()), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }
}
