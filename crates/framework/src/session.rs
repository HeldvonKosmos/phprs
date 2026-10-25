//! Sessions (ROADMAP §3 "Sessions", not in Loco): cookie `opengrid_session`,
//! data lives in the worker during the request and in a store between
//! requests. Store: memory (per process) or the SurrealDB table
//! `_opengrid_sessions`. Flash data survives exactly one more request.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::Vm;
use surrealdb::types::{Datetime, Number, Object, RecordId, RecordIdKey, Value};

use crate::http::HttpResponse;
use crate::native::ctx;

pub const TABLE: &str = "_opengrid_sessions";
const FLASH_KEY: &str = "_flash_new";
/// SES1: serialized objects of `Session::remember`, key → `serialize()` form.
pub const OBJECTS_KEY: &str = "_objects";

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub driver: SessionDriver,
    pub cookie: String,
    pub lifetime: Duration,
    pub secure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDriver {
    Memory,
    Database,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig { driver: SessionDriver::Memory, cookie: "opengrid_session".into(), lifetime: Duration::from_secs(120 * 60), secure: false }
    }
}

impl SessionConfig {
    /// `settings.opengrid.session.{driver, cookie, lifetime (minutes), secure}`.
    pub fn from_settings(settings: Option<&serde_json::Value>) -> SessionConfig {
        let mut cfg = SessionConfig::default();
        let Some(s) = settings.and_then(|s| s.get("opengrid")).and_then(|o| o.get("session")) else { return cfg };
        if let Some(d) = s.get("driver").and_then(|v| v.as_str()) {
            cfg.driver = if d.eq_ignore_ascii_case("database") { SessionDriver::Database } else { SessionDriver::Memory };
        }
        if let Some(c) = s.get("cookie").and_then(|v| v.as_str()) {
            cfg.cookie = c.to_string();
        }
        if let Some(m) = s.get("lifetime").and_then(|v| v.as_u64()) {
            cfg.lifetime = Duration::from_secs(m * 60);
        }
        if let Some(b) = s.get("secure").and_then(|v| v.as_bool()) {
            cfg.secure = b;
        }
        cfg
    }
}

/// Between requests: id → JSON payload. Shared by all workers of a pool.
pub struct SessionStore {
    pub config: SessionConfig,
    memory: Mutex<HashMap<String, (String, Instant)>>,
}

impl SessionStore {
    pub fn new(config: SessionConfig) -> Self {
        SessionStore { config, memory: Mutex::new(HashMap::new()) }
    }

    fn load(&self, vm: &mut Vm, id: &str) -> Result<Option<String>, Throw> {
        match self.config.driver {
            SessionDriver::Memory => {
                let mut m = self.memory.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                m.retain(|_, (_, exp)| *exp > now);
                Ok(m.get(id).map(|(p, _)| p.clone()))
            }
            SessionDriver::Database => {
                let mut vars = BTreeMap::new();
                vars.insert("rid".to_string(), Value::RecordId(RecordId::new(TABLE, RecordIdKey::String(id.to_string()))));
                let rows = crate::native::database::query_one(vm, "SELECT payload, expires_at FROM $rid", vars)?;
                let Value::Array(a) = rows else { return Ok(None) };
                let Some(Value::Object(o)) = a.into_inner().into_iter().next() else { return Ok(None) };
                if let Some(Value::Datetime(exp)) = o.get("expires_at")
                    && exp.clone().into_inner() < chrono::Utc::now()
                {
                    return Ok(None);
                }
                Ok(match o.get("payload") {
                    Some(Value::String(p)) => Some(p.clone()),
                    _ => None,
                })
            }
        }
    }

    fn save(&self, vm: &mut Vm, id: &str, payload: String) -> Result<(), Throw> {
        match self.config.driver {
            SessionDriver::Memory => {
                let mut m = self.memory.lock().unwrap_or_else(|e| e.into_inner());
                m.insert(id.to_string(), (payload, Instant::now() + self.config.lifetime));
                Ok(())
            }
            SessionDriver::Database => {
                let mut obj = Object::new();
                obj.insert("payload", Value::String(payload));
                let expires = chrono::Utc::now() + chrono::Duration::from_std(self.config.lifetime).unwrap_or(chrono::Duration::hours(2));
                obj.insert("expires_at", Value::Datetime(Datetime::from(expires)));
                obj.insert("updated_at", Value::Datetime(Datetime::now()));
                let mut vars = BTreeMap::new();
                vars.insert("rid".to_string(), Value::RecordId(RecordId::new(TABLE, RecordIdKey::String(id.to_string()))));
                vars.insert("data".to_string(), Value::Object(obj));
                crate::native::database::query(vm, "UPSERT $rid CONTENT $data", vars)?;
                Ok(())
            }
        }
    }

    fn delete(&self, vm: &mut Vm, id: &str) -> Result<(), Throw> {
        match self.config.driver {
            SessionDriver::Memory => {
                self.memory.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
                Ok(())
            }
            SessionDriver::Database => {
                let mut vars = BTreeMap::new();
                vars.insert("rid".to_string(), Value::RecordId(RecordId::new(TABLE, RecordIdKey::String(id.to_string()))));
                crate::native::database::query(vm, "DELETE $rid", vars)?;
                Ok(())
            }
        }
    }

    pub fn count(&self) -> usize {
        self.memory.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The request's session while it is being handled.
pub struct SessionState {
    pub id: String,
    pub is_new: bool,
    pub data: PhpArray,
    /// Flash keys from the previous request: visible now, gone after save.
    pub flash_old: Vec<String>,
    /// Flash keys set in this request: visible in the next one.
    pub flash_new: Vec<String>,
    pub dirty: bool,
    pub destroyed: bool,
    /// SES1: objects of `Session::remember` in this request; serialized
    /// again at the end, so changes made during the request are kept.
    pub remembered: Vec<(String, grd_vm::value::ObjHandle)>,
}

pub fn new_id() -> String {
    let mut bytes = [0u8; 20];
    if getrandom::fill(&mut bytes).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        bytes[..16].copy_from_slice(&t.to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Cookie header → (name, value) pairs.
pub fn parse_cookies(header: &str) -> Vec<(String, String)> {
    header
        .split(';')
        .filter_map(|p| {
            let (k, v) = p.trim().split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Load (or start) the session of the current request. Lazy: only when
/// dialect code touches `Session`.
pub fn ensure(vm: &mut Vm) -> Result<&mut SessionState, Throw> {
    let (has, cookie_id) = {
        let c = ctx(vm);
        let s = c.request.as_ref().ok_or_else(|| Throw::error("no active request"))?;
        let id = s.req.header("cookie").and_then(|h| parse_cookies(h).into_iter().find(|(k, _)| *k == c.sessions.config.cookie).map(|(_, v)| v));
        (s.session.is_some(), id)
    };
    if !has {
        let store = ctx(vm).sessions.clone();
        let (id, is_new, payload) = match cookie_id.filter(|id| id.len() >= 16 && id.chars().all(|ch| ch.is_ascii_hexdigit())) {
            Some(id) => {
                let payload = store.load(vm, &id)?;
                let is_new = payload.is_none();
                (if is_new { new_id() } else { id }, is_new, payload)
            }
            None => (new_id(), true, None),
        };
        let mut data = PhpArray::new();
        let mut flash_old = Vec::new();
        if let Some(p) = payload
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&p)
            && let Val::Array(arr) = grd_vm::json::from_json(vm, json)?
        {
            data = (*arr).clone();
            if let Some(Val::Array(keys)) = data.remove(&ArrayKey::from_str(FLASH_KEY)) {
                flash_old = keys.values().filter_map(|k| k.to_php_string().ok()).map(|s| s.to_string_lossy()).collect();
            }
        }
        let state = SessionState { id, is_new, data, flash_old, flash_new: Vec::new(), dirty: false, destroyed: false, remembered: Vec::new() };
        ctx(vm).request.as_mut().expect("request").session = Some(state);
    }
    Ok(ctx(vm).request.as_mut().expect("request").session.as_mut().expect("session"))
}

/// Persist the session (if touched) and set the cookie on a new id.
pub fn finish(vm: &mut Vm, resp: &mut HttpResponse) -> Result<(), Throw> {
    let Some(mut state) = ctx(vm).request.as_mut().and_then(|s| s.session.take()) else { return Ok(()) };
    let store = ctx(vm).sessions.clone();
    if state.destroyed {
        store.delete(vm, &state.id)?;
        resp.headers.push(("set-cookie".into(), format!("{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax", store.config.cookie)));
        return Ok(());
    }
    if !(state.dirty || state.is_new || !state.flash_old.is_empty() || !state.remembered.is_empty()) {
        return Ok(());
    }
    // consumed flash data goes; re-flashed keys stay
    for k in &state.flash_old {
        if !state.flash_new.contains(k) {
            state.data.remove(&ArrayKey::from_str(k));
        }
    }
    if !state.remembered.is_empty() {
        let mut objects = match state.data.get(&ArrayKey::from_str(OBJECTS_KEY)) {
            Some(Val::Array(a)) => (**a).clone(),
            _ => PhpArray::new(),
        };
        for (key, h) in std::mem::take(&mut state.remembered) {
            let form = vm.call_value(&Val::str("serialize"), vec![Val::Object(h)])?;
            objects.insert(ArrayKey::from_str(&key), form);
        }
        state.data.insert(ArrayKey::from_str(OBJECTS_KEY), Val::array(objects));
    }
    let mut data = state.data.clone();
    data.insert(ArrayKey::from_str(FLASH_KEY), Val::array(PhpArray::from_values(state.flash_new.iter().map(|k| Val::str(k)))));
    let payload = String::from_utf8_lossy(&grd_vm::json::to_json(vm, &Val::array(data), false)?).into_owned();
    store.save(vm, &state.id, payload)?;
    if state.is_new {
        let mut cookie = format!("{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax", store.config.cookie, state.id, store.config.lifetime.as_secs());
        if store.config.secure {
            cookie.push_str("; Secure");
        }
        resp.headers.push(("set-cookie".into(), cookie));
    }
    Ok(())
}

/// CSRF token of the session (created on first use).
pub fn token(vm: &mut Vm) -> Result<String, Throw> {
    let s = ensure(vm)?;
    if let Some(Val::Str(t)) = s.data.get(&ArrayKey::from_str("_token")) {
        return Ok(t.to_string_lossy());
    }
    let t = new_id();
    s.data.insert(ArrayKey::from_str("_token"), Val::str(&t));
    s.dirty = true;
    Ok(t)
}

pub fn number_val(n: &Number) -> Val {
    match n {
        Number::Int(i) => Val::Int(*i),
        Number::Float(f) => Val::Float(*f),
        Number::Decimal(d) => Val::Float(d.to_string().parse().unwrap_or(0.0)),
    }
}
