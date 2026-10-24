//! Per-opcode execution counts and time (feature `opstats`, PERF1): the
//! container does not allow `perf`, so the interpreter measures itself.
//! Time per op includes builtins and frame setup done by that op, not the
//! execution of a called function's body (that runs as its own ops).

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::Discriminant;
use std::time::Duration;

use crate::bytecode::Op;

thread_local! {
    static STATS: RefCell<HashMap<Discriminant<Op>, (String, u64, Duration)>> = RefCell::new(HashMap::new());
}

pub fn record(op: &Op, took: Duration) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        let e = s.entry(std::mem::discriminant(op)).or_insert_with(|| {
            let d = format!("{op:?}");
            (d.split(|c: char| c == '(' || c == ' ' || c == '{').next().unwrap_or(&d).to_string(), 0, Duration::ZERO)
        });
        e.1 += 1;
        e.2 += took;
    });
}

/// Ranking by time: name, count, total ms, ns per op, share.
pub fn report() -> String {
    STATS.with(|s| {
        let s = s.borrow();
        let total: Duration = s.values().map(|v| v.2).sum();
        let mut rows: Vec<_> = s.values().collect();
        rows.sort_by(|a, b| b.2.cmp(&a.2));
        let mut out = format!("{:<22}{:>14}{:>12}{:>10}{:>8}\n", "op", "count", "ms", "ns/op", "share");
        for (name, n, d) in rows.into_iter().take(30) {
            out.push_str(&format!(
                "{:<22}{:>14}{:>12.1}{:>10.1}{:>7.1}%\n",
                name,
                n,
                d.as_secs_f64() * 1e3,
                d.as_nanos() as f64 / *n as f64,
                100.0 * d.as_secs_f64() / total.as_secs_f64().max(1e-12)
            ));
        }
        out.push_str(&format!("total op time {:.1} ms\n", total.as_secs_f64() * 1e3));
        out
    })
}
