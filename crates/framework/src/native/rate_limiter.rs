//! `OpenGrid\Support\RateLimiter` (AU2), Laravel's `RateLimiter` facade:
//! counters per key with a decay window, shared by all workers of the pool
//! (one process, D5). `hit($key, $decaySeconds)` counts, `tooManyAttempts`,
//! `availableIn`, `remaining`, `clear`, `attempt($key, $max, $fn)`.
//! In memory: counters do not survive a restart, like Laravel's array store.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{ctx, int_arg_opt, string_arg};

pub const CLASS: &str = "OpenGrid\\Support\\RateLimiter";

/// Above this many keys, expired windows are swept on the next hit.
const SWEEP_AT: usize = 10_000;

#[derive(Default)]
pub struct RateLimits {
    counters: Mutex<HashMap<String, Window>>,
}

struct Window {
    attempts: i64,
    resets_at: Instant,
}

impl RateLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count `amount` attempts; a new window starts when none is running.
    pub fn increment(&self, key: &str, decay: Duration, amount: i64) -> i64 {
        let now = Instant::now();
        let mut map = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= SWEEP_AT {
            map.retain(|_, w| w.resets_at > now);
        }
        let w = map.entry(key.to_string()).or_insert(Window { attempts: 0, resets_at: now + decay });
        if w.resets_at <= now {
            *w = Window { attempts: 0, resets_at: now + decay };
        }
        w.attempts += amount;
        w.attempts
    }

    pub fn attempts(&self, key: &str) -> i64 {
        let map = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        map.get(key).filter(|w| w.resets_at > Instant::now()).map_or(0, |w| w.attempts)
    }

    /// Seconds until the window of `key` ends (0 when none is running).
    pub fn available_in(&self, key: &str) -> i64 {
        let map = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.get(key).filter(|w| w.resets_at > now).map_or(0, |w| (w.resets_at - now).as_secs_f64().ceil() as i64)
    }

    pub fn clear(&self, key: &str) {
        self.counters.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    }
}

fn decay(c: &Call, i: usize) -> Result<Duration, Throw> {
    Ok(Duration::from_secs(int_arg_opt(c, i, 60)?.max(1) as u64))
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("hit", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::hit")?;
            let d = decay(&c, 1)?;
            Ok(Val::Int(ctx(vm).limits.increment(&key, d, 1)))
        })
        .static_method("increment", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::increment")?;
            let d = decay(&c, 1)?;
            let amount = int_arg_opt(&c, 2, 1)?;
            Ok(Val::Int(ctx(vm).limits.increment(&key, d, amount)))
        })
        .static_method("attempts", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::attempts")?;
            Ok(Val::Int(ctx(vm).limits.attempts(&key)))
        })
        .static_method("tooManyAttempts", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::tooManyAttempts")?;
            let max = int_arg_opt(&c, 1, 60)?;
            Ok(Val::Bool(ctx(vm).limits.attempts(&key) >= max))
        })
        .static_method("remaining", |vm, c| remaining(vm, &c))
        .static_method("retriesLeft", |vm, c| remaining(vm, &c))
        .static_method("availableIn", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::availableIn")?;
            Ok(Val::Int(ctx(vm).limits.available_in(&key)))
        })
        .static_method("clear", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::clear")?;
            ctx(vm).limits.clear(&key);
            Ok(Val::Null)
        })
        .static_method("resetAttempts", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::resetAttempts")?;
            ctx(vm).limits.clear(&key);
            Ok(Val::Null)
        })
        // attempt($key, $maxAttempts, $callback, $decaySeconds = 60): false when
        // the limit is reached, else counts and returns the callback's result
        // (true when it returns null)
        .static_method("attempt", |vm, c| {
            let key = string_arg(&c, 0, "RateLimiter::attempt")?;
            let max = int_arg_opt(&c, 1, 60)?;
            let d = decay(&c, 3)?;
            if ctx(vm).limits.attempts(&key) >= max {
                return Ok(Val::Bool(false));
            }
            ctx(vm).limits.increment(&key, d, 1);
            let result = vm.call_value(&c.arg(2), Vec::new())?;
            Ok(if result.is_null() { Val::Bool(true) } else { result })
        });
    vm.register_class(def)?;
    Ok(())
}

fn remaining(vm: &mut Vm, c: &Call) -> Result<Val, Throw> {
    let key = string_arg(c, 0, "RateLimiter::remaining")?;
    let max = int_arg_opt(c, 1, 60)?;
    Ok(Val::Int((max - ctx(vm).limits.attempts(&key)).max(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_counts_and_expires() {
        let l = RateLimits::new();
        assert_eq!(l.increment("k", Duration::from_secs(60), 1), 1);
        assert_eq!(l.increment("k", Duration::from_secs(60), 2), 3);
        assert_eq!(l.attempts("k"), 3);
        assert!((59..=60).contains(&l.available_in("k")));
        l.clear("k");
        assert_eq!((l.attempts("k"), l.available_in("k")), (0, 0));
        l.increment("short", Duration::from_millis(20), 1);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(l.attempts("short"), 0, "window ended");
        assert_eq!(l.increment("short", Duration::from_secs(60), 1), 1, "new window starts at one");
    }
}
