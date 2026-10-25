//! Dates: `date`, `time`, `mktime`, `strtotime`, `checkdate`, the helpers
//! (`__grd_date_*`) still used by the prelude's DateTimeZone/DateInterval, and
//! the Rust functions behind the native DateTime classes (`datetime_class`).

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike, Weekday};
use chrono_tz::OffsetName;

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

#[derive(Clone)]
pub(crate) enum Zone {
    Named(chrono_tz::Tz),
    Fixed(FixedOffset),
}

impl Zone {
    pub(crate) fn parse(name: &str) -> Option<Zone> {
        let n = name.trim();
        if n.eq_ignore_ascii_case("utc") || n.eq_ignore_ascii_case("z") || n.eq_ignore_ascii_case("gmt") {
            return Some(Zone::Named(chrono_tz::UTC));
        }
        if let Some(off) = parse_offset(n) {
            return Some(Zone::Fixed(off));
        }
        n.parse::<chrono_tz::Tz>().ok().map(Zone::Named)
    }
    pub(crate) fn name(&self) -> String {
        match self {
            Zone::Named(tz) => tz.name().to_string(),
            Zone::Fixed(o) => format_offset(o.local_minus_utc(), true),
        }
    }
    pub(crate) fn offset_at(&self, ts: i64) -> i32 {
        match self {
            Zone::Named(tz) => tz.offset_from_utc_datetime(&DateTime::from_timestamp(ts, 0).unwrap_or_default().naive_utc()).fix().local_minus_utc(),
            Zone::Fixed(o) => o.local_minus_utc(),
        }
    }
    fn to_local(&self, ts: i64) -> NaiveDateTime {
        let utc = DateTime::from_timestamp(ts, 0).unwrap_or_default().naive_utc();
        utc + Duration::seconds(self.offset_at(ts) as i64)
    }
    fn from_local(&self, local: NaiveDateTime) -> i64 {
        match self {
            Zone::Named(tz) => match tz.from_local_datetime(&local).earliest() {
                Some(dt) => dt.timestamp(),
                None => local.and_utc().timestamp() - self.offset_at(local.and_utc().timestamp()) as i64,
            },
            Zone::Fixed(o) => local.and_utc().timestamp() - o.local_minus_utc() as i64,
        }
    }
    fn abbreviation(&self, ts: i64) -> String {
        match self {
            Zone::Named(tz) => tz.offset_from_utc_datetime(&DateTime::from_timestamp(ts, 0).unwrap_or_default().naive_utc()).abbreviation().unwrap_or("UTC").to_string(),
            Zone::Fixed(o) => format_offset(o.local_minus_utc(), true),
        }
    }
}

fn parse_offset(s: &str) -> Option<FixedOffset> {
    let b = s.as_bytes();
    if b.len() < 2 || !(b[0] == b'+' || b[0] == b'-') {
        return None;
    }
    let sign = if b[0] == b'-' { -1 } else { 1 };
    let rest: String = s[1..].chars().filter(|c| *c != ':').collect();
    if !rest.chars().all(|c| c.is_ascii_digit()) || rest.is_empty() {
        return None;
    }
    let (h, m) = match rest.len() {
        1 | 2 => (rest.parse::<i32>().ok()?, 0),
        3 => (rest[..1].parse::<i32>().ok()?, rest[1..].parse::<i32>().ok()?),
        4 => (rest[..2].parse::<i32>().ok()?, rest[2..].parse::<i32>().ok()?),
        _ => return None,
    };
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

fn format_offset(secs: i32, colon: bool) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    if colon { format!("{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60) } else { format!("{sign}{:02}{:02}", a / 3600, (a % 3600) / 60) }
}

fn zone_arg(vm: &Vm, a: &[Val], i: usize) -> Result<Zone, Throw> {
    let name = match a.get(i) {
        Some(Val::Str(s)) if !s.is_empty() => s.to_string_lossy(),
        _ => vm.default_timezone.clone(),
    };
    Zone::parse(&name).ok_or_else(|| Throw::new("Exception", format!("DateTimeZone::__construct(): Unknown or bad timezone ({name})")))
}

pub fn register(vm: &mut Vm) {
    vm.register_native("__grd_now", |_, _| {
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        Ok(Val::array(PhpArray::from_values([Val::Int(d.as_secs() as i64), Val::Int(d.subsec_micros() as i64)])))
    });
    vm.register_native("time", |_, _| Ok(Val::Int(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)));
    vm.register_native("microtime", |_, a| {
        // monotonic, arbitrary origin (as in PHP): unaffected by changes of the system clock
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let d = ORIGIN.get_or_init(std::time::Instant::now).elapsed();
        if args::boolean(a, 0, false)? {
            Ok(Val::Float(d.as_secs_f64()))
        } else {
            Ok(Val::from(format!("{:.8} {}", d.subsec_nanos() as f64 / 1e9, d.as_secs())))
        }
    });
    vm.register_native("hrtime", |_, a| {
        let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        if args::boolean(a, 0, false)? {
            Ok(Val::Int(d.as_nanos() as i64))
        } else {
            Ok(Val::array(PhpArray::from_values([Val::Int(d.as_secs() as i64), Val::Int(d.subsec_nanos() as i64)])))
        }
    });
    vm.register_native("date_default_timezone_set", |vm, a| {
        let n = args::string(a, 0, "date_default_timezone_set")?.to_string_lossy();
        if Zone::parse(&n).is_none() {
            return Err(Throw::value_error(format!("date_default_timezone_set(): Argument #1 ($timezoneId) must be a valid timezone, \"{n}\" given")));
        }
        vm.default_timezone = n;
        Ok(Val::Bool(true))
    });
    vm.register_native("date_default_timezone_get", |vm, _| Ok(Val::from(vm.default_timezone.clone())));
    vm.register_native("__grd_tz_valid", |_, a| Ok(Val::Bool(Zone::parse(&args::string(a, 0, "tz")?.to_string_lossy()).is_some())));
    vm.register_native("__grd_tz_name", |vm, a| Ok(Val::from(zone_arg(vm, a, 0)?.name())));
    vm.register_native("__grd_tz_offset", |vm, a| {
        let z = zone_arg(vm, a, 0)?;
        let ts = args::int_opt(a, 1, "tz", 0)?;
        Ok(Val::Int(z.offset_at(ts) as i64))
    });
    vm.register_native("date", |vm, a| {
        let fmt = args::string(a, 0, "date")?;
        let ts = match a.get(1) { None | Some(Val::Null) => now_secs(), _ => args::int(a, 1, "date")? };
        let z = zone_arg(vm, &[], 0)?;
        Ok(Val::from(format_date(&fmt.to_string_lossy(), ts, 0, &z)))
    });
    vm.register_native("gmdate", |_, a| {
        let fmt = args::string(a, 0, "gmdate")?;
        let ts = match a.get(1) { None | Some(Val::Null) => now_secs(), _ => args::int(a, 1, "gmdate")? };
        Ok(Val::from(format_date(&fmt.to_string_lossy(), ts, 0, &Zone::Named(chrono_tz::UTC))))
    });
    vm.register_native("idate", |vm, a| {
        let fmt = args::string(a, 0, "idate")?;
        let ts = match a.get(1) { None | Some(Val::Null) => now_secs(), _ => args::int(a, 1, "idate")? };
        let z = zone_arg(vm, &[], 0)?;
        Ok(Val::Int(format_date(&fmt.to_string_lossy(), ts, 0, &z).parse().unwrap_or(0)))
    });
    vm.register_native("mktime", |vm, a| {
        let z = zone_arg(vm, &[], 0)?;
        let now = z.to_local(now_secs());
        let get = |i: usize, d: i64| -> Result<i64, Throw> { match a.get(i) { None | Some(Val::Null) => Ok(d), _ => args::int(a, i, "mktime") } };
        let (h, mi, s) = (get(0, now.hour() as i64)?, get(1, now.minute() as i64)?, get(2, now.second() as i64)?);
        let (mo, d, y) = (get(3, now.month() as i64)?, get(4, now.day() as i64)?, get(5, now.year() as i64)?);
        Ok(Val::Int(make_time(&z, y, mo, d, h, mi, s)))
    });
    vm.register_native("gmmktime", |_, a| {
        let z = Zone::Named(chrono_tz::UTC);
        let now = z.to_local(now_secs());
        let get = |i: usize, d: i64| -> Result<i64, Throw> { match a.get(i) { None | Some(Val::Null) => Ok(d), _ => args::int(a, i, "gmmktime") } };
        let (h, mi, s) = (get(0, now.hour() as i64)?, get(1, now.minute() as i64)?, get(2, now.second() as i64)?);
        let (mo, d, y) = (get(3, now.month() as i64)?, get(4, now.day() as i64)?, get(5, now.year() as i64)?);
        Ok(Val::Int(make_time(&z, y, mo, d, h, mi, s)))
    });
    vm.register_native("checkdate", |_, a| {
        let (m, d, y) = (args::int(a, 0, "checkdate")?, args::int(a, 1, "checkdate")?, args::int(a, 2, "checkdate")?);
        Ok(Val::Bool(NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32).is_some() && (1..=12).contains(&m) && d >= 1))
    });
    vm.register_native("strtotime", |vm, a| {
        let s = args::string(a, 0, "strtotime")?;
        let base = match a.get(1) { None | Some(Val::Null) => now_secs(), _ => args::int(a, 1, "strtotime")? };
        let z = zone_arg(vm, &[], 0)?;
        Ok(match parse_datetime(&s.to_string_lossy(), base, &z) {
            Some((ts, _)) => Val::Int(ts),
            None => Val::Bool(false),
        })
    });
    vm.register_native("__grd_interval_parse", |_, a| {
        let s = args::string(a, 0, "DateInterval")?.to_string_lossy();
        parse_iso_interval(&s).map(|v| Val::array(PhpArray::from_values(v.into_iter().map(Val::Int)))).ok_or_else(|| Throw::new("Exception", format!("DateInterval::__construct(): Unknown or bad format ({s})")))
    });
    vm.register_native("__grd_relative_parts", |_, a| {
        // "+1 day 2 hours" → [y, m, d, h, i, s] or false
        let s = args::string(a, 0, "DateInterval")?.to_string_lossy();
        Ok(match parse_relative_only(&s) {
            Some(v) => Val::array(PhpArray::from_values(v.into_iter().map(Val::Int))),
            None => Val::Bool(false),
        })
    });
}

/// `(seconds, microseconds)` of the wall clock.
pub(crate) fn now_micros() -> (i64, i64) {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() as i64, d.subsec_micros() as i64)
}

/// `ts` moved by `[y, m, d, h, i, s] * sign` in zone `z` (calendar arithmetic).
/// DateTime::add/sub as PHP (timelib's add_wall/sub_wall): years, months and
/// days move the local wall clock; hours, minutes and seconds are elapsed
/// time added to the instant, so they cross a DST change correctly
/// (03:00 EDT - 1 hour = 01:00 EST, not the missing 02:00).
pub(crate) fn date_add(ts: i64, parts: [i64; 6], sign: i64, z: &Zone) -> i64 {
    let v: Vec<i64> = parts.iter().map(|x| x * sign).collect();
    let ts = if v[0] != 0 || v[1] != 0 || v[2] != 0 {
        let l = z.to_local(ts);
        z.from_local(add_months(l, v[0] * 12 + v[1]) + Duration::days(v[2]))
    } else {
        ts
    };
    ts + v[3] * 3600 + v[4] * 60 + v[5]
}

/// `[y, m, d, h, i, s, days, invert]` between two instants, like DateTime::diff.
pub(crate) fn date_diff(t1: i64, t2: i64, z: &Zone) -> [i64; 8] {
    let (from, to, invert) = if t1 <= t2 { (t1, t2, 0) } else { (t2, t1, 1) };
    let (a1, a2) = (z.to_local(from), z.to_local(to));
    let mut y = a2.year() - a1.year();
    let mut m = a2.month() as i32 - a1.month() as i32;
    let mut d = a2.day() as i32 - a1.day() as i32;
    let mut secs = a2.num_seconds_from_midnight() as i64 - a1.num_seconds_from_midnight() as i64;
    if secs < 0 {
        secs += 86400;
        d -= 1;
    }
    if d < 0 {
        // borrow days of the previous month of `to`
        let prev = add_months(a2, -1);
        d += days_in_month(prev.year(), prev.month()) as i32;
        m -= 1;
    }
    if m < 0 {
        m += 12;
        y -= 1;
    }
    let days = (to - from) / 86400;
    [y as i64, m as i64, d as i64, secs / 3600, (secs % 3600) / 60, secs % 60, days, invert]
}

/// Replace date/time fields (`None` = keep) in zone `z`.
pub(crate) fn date_set(ts: i64, parts: [Option<i64>; 6], z: &Zone) -> i64 {
    let l = z.to_local(ts);
    let cur = [l.year() as i64, l.month() as i64, l.day() as i64, l.hour() as i64, l.minute() as i64, l.second() as i64];
    let v: Vec<i64> = parts.iter().zip(cur).map(|(p, c)| p.unwrap_or(c)).collect();
    make_time(z, v[0], v[1], v[2], v[3], v[4], v[5])
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    (NaiveDate::from_ymd_opt(ny, nm, 1).unwrap_or_default() - Duration::days(1)).day()
}

/// Month arithmetic with PHP overflow semantics (Jan 31 + 1 month = Mar 3).
fn add_months(dt: NaiveDateTime, months: i64) -> NaiveDateTime {
    let total = dt.year() as i64 * 12 + (dt.month() as i64 - 1) + months;
    let (y, m) = (total.div_euclid(12) as i32, (total.rem_euclid(12) + 1) as u32);
    let first = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_default();
    let overflow = dt.day() as i64 - 1;
    (first + Duration::days(overflow)).and_time(dt.time())
}

fn make_time(z: &Zone, y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    // normalise overflowing fields like mktime does
    let base = NaiveDate::from_ymd_opt(y as i32, 1, 1).unwrap_or_default().and_hms_opt(0, 0, 0).unwrap_or_default();
    let dt = add_months(base, mo - 1) + Duration::days(d - 1) + Duration::hours(h) + Duration::minutes(mi) + Duration::seconds(s);
    z.from_local(dt)
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub(crate) fn format_date(fmt: &str, ts: i64, us: i64, z: &Zone) -> String {
    let l = z.to_local(ts);
    let off = z.offset_at(ts);
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            'd' => out.push_str(&format!("{:02}", l.day())),
            'D' => out.push_str(&DAYS[l.weekday().num_days_from_sunday() as usize][..3]),
            'j' => out.push_str(&l.day().to_string()),
            'l' => out.push_str(DAYS[l.weekday().num_days_from_sunday() as usize]),
            'N' => out.push_str(&l.weekday().number_from_monday().to_string()),
            'S' => out.push_str(match l.day() { 1 | 21 | 31 => "st", 2 | 22 => "nd", 3 | 23 => "rd", _ => "th" }),
            'w' => out.push_str(&l.weekday().num_days_from_sunday().to_string()),
            'z' => out.push_str(&l.ordinal0().to_string()),
            'W' => out.push_str(&format!("{:02}", l.iso_week().week())),
            'F' => out.push_str(MONTHS[l.month0() as usize]),
            'm' => out.push_str(&format!("{:02}", l.month())),
            'M' => out.push_str(&MONTHS[l.month0() as usize][..3]),
            'n' => out.push_str(&l.month().to_string()),
            't' => out.push_str(&days_in_month(l.year(), l.month()).to_string()),
            'L' => out.push(if NaiveDate::from_ymd_opt(l.year(), 2, 29).is_some() { '1' } else { '0' }),
            'o' => out.push_str(&l.iso_week().year().to_string()),
            'Y' => out.push_str(&l.year().to_string()),
            'y' => out.push_str(&format!("{:02}", l.year().rem_euclid(100))),
            'a' => out.push_str(if l.hour() < 12 { "am" } else { "pm" }),
            'A' => out.push_str(if l.hour() < 12 { "AM" } else { "PM" }),
            'g' => out.push_str(&(if l.hour() % 12 == 0 { 12 } else { l.hour() % 12 }).to_string()),
            'G' => out.push_str(&l.hour().to_string()),
            'h' => out.push_str(&format!("{:02}", if l.hour() % 12 == 0 { 12 } else { l.hour() % 12 })),
            'H' => out.push_str(&format!("{:02}", l.hour())),
            'i' => out.push_str(&format!("{:02}", l.minute())),
            's' => out.push_str(&format!("{:02}", l.second())),
            'u' => out.push_str(&format!("{:06}", us)),
            'v' => out.push_str(&format!("{:03}", us / 1000)),
            'e' => out.push_str(&z.name()),
            'I' => out.push('0'),
            'O' => out.push_str(&format_offset(off, false)),
            'P' => out.push_str(&format_offset(off, true)),
            'p' => {
                if off == 0 {
                    out.push('Z');
                } else {
                    out.push_str(&format_offset(off, true));
                }
            }
            'T' => out.push_str(&z.abbreviation(ts)),
            'Z' => out.push_str(&off.to_string()),
            'c' => out.push_str(&format_date("Y-m-d\\TH:i:sP", ts, us, z)),
            'r' => out.push_str(&format_date("D, d M Y H:i:s O", ts, us, z)),
            'U' => out.push_str(&ts.to_string()),
            other => out.push(other),
        }
    }
    out
}

fn weekday_from(name: &str) -> Option<Weekday> {
    let n = name.to_ascii_lowercase();
    let n = n.trim_end_matches('s');
    Some(match &n[..n.len().min(3)] {
        "mon" => Weekday::Mon,
        "tue" => Weekday::Tue,
        "wed" => Weekday::Wed,
        "thu" => Weekday::Thu,
        "fri" => Weekday::Fri,
        "sat" => Weekday::Sat,
        "sun" => Weekday::Sun,
        _ => return None,
    })
}

fn month_from(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    if n.len() < 3 {
        return None;
    }
    MONTHS.iter().position(|m| m.to_ascii_lowercase().starts_with(&n[..3]) && m.to_ascii_lowercase().starts_with(&n)).map(|i| i as u32 + 1)
        .or_else(|| MONTHS.iter().position(|m| m.to_ascii_lowercase().starts_with(&n[..3])).filter(|_| n.len() == 3).map(|i| i as u32 + 1))
}

fn unit_seconds(unit: &str) -> Option<(i64, i64, i64)> {
    // (months, days, seconds) per unit
    let u = unit.to_ascii_lowercase();
    let u = u.trim_end_matches('s');
    Some(match u {
        "sec" | "second" => (0, 0, 1),
        "min" | "minute" => (0, 0, 60),
        "hour" => (0, 0, 3600),
        "day" => (0, 1, 0),
        "week" => (0, 7, 0),
        "fortnight" => (0, 14, 0),
        "month" => (1, 0, 0),
        "year" => (12, 0, 0),
        _ => return None,
    })
}

/// Parse absolute and relative date strings (a practical subset of PHP's).
/// Returns the timestamp and an explicit zone if the string carried one.
pub(crate) fn parse_datetime(input: &str, base: i64, z: &Zone) -> Option<(i64, Option<Zone>)> {
    let s = input.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("now") {
        return Some((base, None));
    }
    if let Some(rest) = s.strip_prefix('@') {
        return rest.trim().parse::<i64>().ok().map(|t| (t, Some(Zone::Named(chrono_tz::UTC))));
    }
    let mut zone = z.clone();
    let mut explicit_zone: Option<Zone> = None;
    let mut local = zone.to_local(base);
    let mut tokens: Vec<String> = s.split_whitespace().map(str::to_string).collect();
    let mut time_set = false;
    let mut date_set = false;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].clone();
        let tl = t.to_ascii_lowercase();
        // ISO date / datetime
        if let Some((d, rest)) = parse_iso_date(&t) {
            local = d.and_time(local.time());
            date_set = true;
            if let Some(r) = rest {
                tokens.insert(i + 1, r);
            }
            i += 1;
            continue;
        }
        if let Some((time, tz)) = parse_time_token(&t) {
            local = local.date().and_time(time);
            time_set = true;
            if let Some(tz) = tz {
                explicit_zone = Some(tz.clone());
                zone = tz;
            }
            i += 1;
            continue;
        }
        let looks_like_offset = (t.starts_with('+') || t.starts_with('-')) && t[1..].chars().filter(|c| c.is_ascii_digit()).count() >= 4 && t[1..].chars().all(|c| c.is_ascii_digit() || c == ':');
        if let Some(tz) = Zone::parse(&t).filter(|_| t.contains('/') || t.eq_ignore_ascii_case("utc") || t.eq_ignore_ascii_case("gmt") || looks_like_offset) {
            explicit_zone = Some(tz.clone());
            zone = tz;
            i += 1;
            continue;
        }
        match tl.as_str() {
            "now" => {}
            "today" | "midnight" => {
                local = local.date().and_hms_opt(0, 0, 0)?;
                time_set = true;
            }
            "noon" => {
                local = local.date().and_hms_opt(12, 0, 0)?;
                time_set = true;
            }
            "tomorrow" => {
                local = (local.date() + Duration::days(1)).and_hms_opt(0, 0, 0)?;
                time_set = true;
            }
            "yesterday" => {
                local = (local.date() - Duration::days(1)).and_hms_opt(0, 0, 0)?;
                time_set = true;
            }
            "next" | "last" | "this" | "previous" => {
                let dir: i64 = match tl.as_str() { "next" => 1, "this" => 0, _ => -1 };
                let what = tokens.get(i + 1)?.to_ascii_lowercase();
                i += 1;
                if let Some(wd) = weekday_from(&what) {
                    local = move_to_weekday(local, wd, dir);
                    if !time_set {
                        local = local.date().and_hms_opt(0, 0, 0)?;
                        time_set = true;
                    }
                } else if let Some((mo, d, sec)) = unit_seconds(&what) {
                    local = add_months(local, mo * dir) + Duration::days(d * dir) + Duration::seconds(sec * dir);
                } else {
                    return None;
                }
            }
            "first" | "last_" => {}
            _ if tl == "first" => {}
            _ => {
                // "first day of", "last day of"
                if (tl == "first" || tl == "last") && tokens.get(i + 1).is_some_and(|x| x.eq_ignore_ascii_case("day")) && tokens.get(i + 2).is_some_and(|x| x.eq_ignore_ascii_case("of")) {
                    let rest: Vec<String> = tokens[i + 3..].to_vec();
                    let (ts2, _) = parse_datetime(&rest.join(" "), zone.from_local(local), &zone).unwrap_or((zone.from_local(local), None));
                    let l2 = zone.to_local(ts2);
                    let day = if tl == "first" { 1 } else { days_in_month(l2.year(), l2.month()) };
                    local = NaiveDate::from_ymd_opt(l2.year(), l2.month(), day)?.and_time(if time_set { local.time() } else { NaiveTime::from_hms_opt(0, 0, 0)? });
                    break;
                }
                // weekday name alone: next occurrence (today counts)
                if let Some(wd) = weekday_from(&tl) {
                    let mut d = local.date();
                    while d.weekday() != wd {
                        d += Duration::days(1);
                    }
                    local = d.and_hms_opt(0, 0, 0)?;
                    time_set = true;
                    i += 1;
                    continue;
                }
                // "+1 day", "-2 weeks", "3 months", "1 day ago"
                let (sign, num_text) = match tl.strip_prefix('+') {
                    Some(r) => (1i64, r.to_string()),
                    None => match tl.strip_prefix('-') {
                        Some(r) => (-1, r.to_string()),
                        None => (1, tl.clone()),
                    },
                };
                if let Ok(n) = num_text.parse::<i64>() {
                    let unit = tokens.get(i + 1)?.to_ascii_lowercase();
                    let (mo, d, sec) = unit_seconds(&unit)?;
                    let mut mult = sign;
                    if tokens.get(i + 2).is_some_and(|x| x.eq_ignore_ascii_case("ago")) {
                        mult = -mult;
                        i += 1;
                    }
                    local = add_months(local, mo * n * mult) + Duration::days(d * n * mult) + Duration::seconds(sec * n * mult);
                    i += 2;
                    continue;
                }
                // "<Month> <day>, <year>" / "<day> <Month> <year>"
                if let Some(mo) = month_from(&tl) {
                    let mut day = 1u32;
                    let mut year = local.year();
                    let mut consumed = 1;
                    if let Some(dtok) = tokens.get(i + 1) {
                        let dclean = dtok.trim_end_matches(',').trim_end_matches(|c: char| c.is_alphabetic());
                        if let Ok(d) = dclean.parse::<u32>() {
                            day = d;
                            consumed += 1;
                            if let Some(ytok) = tokens.get(i + 2)
                                && let Ok(y) = ytok.parse::<i32>()
                            {
                                year = y;
                                consumed += 1;
                            }
                        }
                    }
                    local = NaiveDate::from_ymd_opt(year, mo, day)?.and_time(local.time());
                    date_set = true;
                    i += consumed;
                    continue;
                }
                if let Ok(d) = tl.parse::<u32>()
                    && let Some(mtok) = tokens.get(i + 1)
                    && let Some(mo) = month_from(mtok)
                {
                    let mut year = local.year();
                    let mut consumed = 2;
                    if let Some(ytok) = tokens.get(i + 2)
                        && let Ok(y) = ytok.parse::<i32>()
                    {
                        year = y;
                        consumed += 1;
                    }
                    local = NaiveDate::from_ymd_opt(year, mo, d)?.and_time(local.time());
                    date_set = true;
                    i += consumed;
                    continue;
                }
                return None;
            }
        }
        i += 1;
    }
    if date_set && !time_set {
        local = local.date().and_hms_opt(0, 0, 0)?;
    }
    Some((zone.from_local(local), explicit_zone))
}

fn move_to_weekday(local: NaiveDateTime, wd: Weekday, dir: i64) -> NaiveDateTime {
    let mut d = local.date();
    match dir {
        1 => {
            d += Duration::days(1);
            while d.weekday() != wd {
                d += Duration::days(1);
            }
        }
        -1 => {
            d -= Duration::days(1);
            while d.weekday() != wd {
                d -= Duration::days(1);
            }
        }
        _ => {
            while d.weekday() != wd {
                d += Duration::days(1);
            }
        }
    }
    d.and_time(local.time())
}

/// `2026-10-23`, `2026/10/23`, `23.10.2026`, `10/23/2026`, `2026-10-23T10:00:00` (rest returned).
fn parse_iso_date(t: &str) -> Option<(NaiveDate, Option<String>)> {
    let (date_part, rest) = match t.find('T') {
        Some(p) if t.len() > p + 1 && t.as_bytes()[p + 1].is_ascii_digit() => (&t[..p], Some(t[p + 1..].to_string())),
        _ => (t, None),
    };
    let b = date_part.as_bytes();
    if b.len() >= 8 && b[4] == b'-' {
        let y: i32 = date_part[..4].parse().ok()?;
        let rest_parts: Vec<&str> = date_part[5..].split('-').collect();
        if rest_parts.len() == 2 {
            let m: u32 = rest_parts[0].parse().ok()?;
            let d: u32 = rest_parts[1].parse().ok()?;
            return NaiveDate::from_ymd_opt(y, m, d).map(|d| (d, rest));
        }
    }
    if b.len() >= 8 && b[4] == b'/' {
        let parts: Vec<&str> = date_part.split('/').collect();
        if parts.len() == 3 {
            return NaiveDate::from_ymd_opt(parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?).map(|d| (d, rest));
        }
    }
    if date_part.contains('.') && !date_part.contains(':') {
        let parts: Vec<&str> = date_part.split('.').collect();
        if parts.len() == 3 && parts[2].len() == 4 {
            return NaiveDate::from_ymd_opt(parts[2].parse().ok()?, parts[1].parse().ok()?, parts[0].parse().ok()?).map(|d| (d, rest));
        }
    }
    if date_part.contains('/') {
        let parts: Vec<&str> = date_part.split('/').collect();
        if parts.len() == 3 && parts[2].len() == 4 {
            return NaiveDate::from_ymd_opt(parts[2].parse().ok()?, parts[0].parse().ok()?, parts[1].parse().ok()?).map(|d| (d, rest));
        }
    }
    if b.len() == 8 && b.iter().all(u8::is_ascii_digit) {
        return NaiveDate::from_ymd_opt(date_part[..4].parse().ok()?, date_part[4..6].parse().ok()?, date_part[6..].parse().ok()?).map(|d| (d, rest));
    }
    None
}

/// `10:00`, `10:00:00`, `10:00:00.123`, `10:00:00Z`, `10:00:00+02:00`, `10am`, `3:30pm`.
fn parse_time_token(t: &str) -> Option<(NaiveTime, Option<Zone>)> {
    let tl = t.to_ascii_lowercase();
    let (body, ampm) = if let Some(b) = tl.strip_suffix("am") { (b.to_string(), Some(false)) } else if let Some(b) = tl.strip_suffix("pm") { (b.to_string(), Some(true)) } else { (tl.clone(), None) };
    let (body, zone) = if let Some(b) = body.strip_suffix('z') {
        (b.to_string(), Some(Zone::Named(chrono_tz::UTC)))
    } else if let Some(p) = body.rfind(['+', '-']).filter(|p| *p > 0 && body.contains(':')) {
        let z = Zone::parse(&body[p..])?;
        (body[..p].to_string(), Some(z))
    } else {
        (body, None)
    };
    if body.is_empty() || !body.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    if !body.contains(':') && ampm.is_none() {
        return None;
    }
    let (main, frac) = match body.split_once('.') {
        Some((m, _)) => (m.to_string(), 0),
        None => (body.clone(), 0),
    };
    let parts: Vec<&str> = main.split(':').collect();
    let mut h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts.get(1).map(|x| x.parse().ok()).unwrap_or(Some(0))?;
    let s: u32 = parts.get(2).map(|x| x.parse().ok()).unwrap_or(Some(0))?;
    if let Some(pm) = ampm {
        if h == 12 {
            h = 0;
        }
        if pm {
            h += 12;
        }
    }
    let _ = frac;
    NaiveTime::from_hms_opt(h, m, s).map(|t| (t, zone))
}

/// `DateTime::createFromFormat` subset: Y m d H i s j n G D M y U, literals.
pub(crate) fn parse_with_format(fmt: &str, s: &str, z: &Zone) -> Option<i64> {
    let now = z.to_local(now_secs());
    let (mut y, mut mo, mut d, mut h, mut mi, mut sec) = (now.year(), now.month(), now.day(), now.hour(), now.minute(), now.second());
    let mut reset_time = false;
    let mut ts_override: Option<i64> = None;
    let sb = s.as_bytes();
    let mut pos = 0usize;
    let read_num = |pos: &mut usize, max: usize| -> Option<i64> {
        let start = *pos;
        while *pos < sb.len() && sb[*pos].is_ascii_digit() && *pos - start < max {
            *pos += 1;
        }
        if *pos == start {
            return None;
        }
        s[start..*pos].parse().ok()
    };
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        match c {
            'Y' => y = read_num(&mut pos, 4)? as i32,
            'y' => y = 2000 + read_num(&mut pos, 2)? as i32,
            'm' | 'n' => mo = read_num(&mut pos, 2)? as u32,
            'd' | 'j' => d = read_num(&mut pos, 2)? as u32,
            'H' | 'G' => h = read_num(&mut pos, 2)? as u32,
            'i' => mi = read_num(&mut pos, 2)? as u32,
            's' => sec = read_num(&mut pos, 2)? as u32,
            'U' => ts_override = Some(read_num(&mut pos, 20)?),
            '!' => {
                y = 1970;
                mo = 1;
                d = 1;
                reset_time = true;
            }
            '|' => reset_time = true,
            'M' | 'D' => {
                let word: String = s[pos..].chars().take_while(|c| c.is_alphabetic()).collect();
                pos += word.len();
                if c == 'M' {
                    mo = month_from(&word)?;
                }
            }
            '\\' => {
                let lit = chars.next()?;
                if s[pos..].starts_with(lit) {
                    pos += lit.len_utf8();
                } else {
                    return None;
                }
            }
            other => {
                if s[pos..].starts_with(other) {
                    pos += other.len_utf8();
                } else {
                    return None;
                }
            }
        }
    }
    if let Some(t) = ts_override {
        return Some(t);
    }
    if reset_time && !fmt.contains('H') && !fmt.contains('G') {
        h = 0;
        mi = 0;
        sec = 0;
    }
    let dt = NaiveDate::from_ymd_opt(y, mo, d)?.and_hms_opt(h, mi, sec)?;
    Some(z.from_local(dt))
}

/// `P1Y2M3DT4H5M6S` → [y, m, d, h, i, s]
fn parse_iso_interval(s: &str) -> Option<Vec<i64>> {
    let s = s.strip_prefix('P')?;
    let mut out = vec![0i64; 6];
    let (date_part, time_part) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut num = String::new();
    for c in date_part.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: i64 = num.parse().ok()?;
            num.clear();
            match c {
                'Y' => out[0] = n,
                'M' => out[1] = n,
                'W' => out[2] += n * 7,
                'D' => out[2] += n,
                _ => return None,
            }
        }
    }
    if let Some(t) = time_part {
        for c in t.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                let n: i64 = num.parse().ok()?;
                num.clear();
                match c {
                    'H' => out[3] = n,
                    'M' => out[4] = n,
                    'S' => out[5] = n,
                    _ => return None,
                }
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(out)
}

/// Relative-only strings for `DateInterval::createFromDateString` and
/// `modify`: `"+1 day"`, `"2 weeks"`, `"3 months 2 days"`.
fn parse_relative_only(s: &str) -> Option<Vec<i64>> {
    let mut out = vec![0i64; 6];
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].to_ascii_lowercase();
        let (sign, n) = match t.strip_prefix('+') {
            Some(r) => (1, r.parse::<i64>().ok()?),
            None => match t.strip_prefix('-') {
                Some(r) => (-1, r.parse::<i64>().ok()?),
                None => (1, t.parse::<i64>().ok()?),
            },
        };
        let unit = tokens.get(i + 1)?.to_ascii_lowercase();
        let (mo, d, sec) = unit_seconds(&unit)?;
        let mut mult = sign;
        if tokens.get(i + 2).is_some_and(|x| x.eq_ignore_ascii_case("ago")) {
            mult = -mult;
            i += 1;
        }
        out[0] += (mo * n * mult) / 12;
        out[1] += (mo * n * mult) % 12;
        out[2] += d * n * mult;
        out[3] += (sec * n * mult) / 3600;
        out[4] += ((sec * n * mult) % 3600) / 60;
        out[5] += (sec * n * mult) % 60;
        i += 2;
    }
    Some(out)
}
