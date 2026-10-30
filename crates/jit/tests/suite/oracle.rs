//! TEST2 (D18): what a test does when an independent oracle is missing
//! (`php`, Laravel's vendor directory, `surreal`, `refs/platform`).
//! With `OPENGRID_REQUIRE_ORACLES=1` (CI, commit hook) the test fails;
//! `OPENGRID_ORACLES_EXCEPT=platform` exempts kinds (comma list; CI has no
//! confidential platform sources). Otherwise the test is skipped with a
//! notice that cannot be overlooked.

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum Oracle {
    Php,
    LaravelVendor,
    Surreal,
    Platform,
}

impl Oracle {
    fn key(self) -> &'static str {
        match self {
            Oracle::Php => "php",
            Oracle::LaravelVendor => "laravel",
            Oracle::Surreal => "surreal",
            Oracle::Platform => "platform",
        }
    }
}

/// Report a missing oracle; panics when oracles are required.
#[allow(dead_code)]
pub fn missing(kind: Oracle, what: &str) {
    let required = std::env::var("OPENGRID_REQUIRE_ORACLES").is_ok_and(|v| v == "1");
    let exempt = std::env::var("OPENGRID_ORACLES_EXCEPT").is_ok_and(|v| v.split(',').any(|k| k.trim() == kind.key()));
    if required && !exempt {
        panic!("ORACLE MISSING ({}): {what} — OPENGRID_REQUIRE_ORACLES=1 requires it", kind.key());
    }
    eprintln!("\n*** SKIPPED ({}): {what} ***\n", kind.key());
}
