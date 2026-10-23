//! The route table a worker fills while running `routes/*.grd`.
//! Path syntax follows matchit: `/tasks/{id}`, `/files/{*path}`.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
    pub class: String,
    pub method: String,
}

#[derive(Default)]
pub struct RouteTable {
    by_method: HashMap<String, matchit::Router<Handler>>,
    entries: Vec<(String, String, Handler)>,
}

impl RouteTable {
    pub fn add(&mut self, method: &str, path: &str, handler: Handler) -> Result<(), String> {
        let method = method.to_ascii_uppercase();
        let path = normalize(path);
        self.by_method
            .entry(method.clone())
            .or_default()
            .insert(path.clone(), handler.clone())
            .map_err(|e| format!("route {method} {path}: {e}"))?;
        self.entries.push((method, path, handler));
        Ok(())
    }

    /// Specific method first, then `ANY`.
    pub fn find(&self, method: &str, path: &str) -> Option<(Handler, Vec<(String, String)>)> {
        let path = normalize(path);
        let method = method.to_ascii_uppercase();
        let method = if method == "HEAD" { "GET".to_string() } else { method };
        for m in [method.as_str(), "ANY"] {
            if let Some(router) = self.by_method.get(m)
                && let Ok(found) = router.at(&path)
            {
                let params = found.params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                return Some((found.value.clone(), params));
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[(String, String, Handler)] {
        &self.entries
    }
}

fn normalize(path: &str) -> String {
    let p = path.trim_end_matches('/');
    if p.is_empty() { "/".to_string() } else if p.starts_with('/') { p.to_string() } else { format!("/{p}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_params_and_any() {
        let mut t = RouteTable::default();
        let h = |m: &str| Handler { class: "C".into(), method: m.into() };
        t.add("get", "/tasks", h("index")).unwrap();
        t.add("GET", "/tasks/{id}", h("show")).unwrap();
        t.add("any", "/ping", h("ping")).unwrap();
        assert_eq!(t.find("GET", "/tasks/").unwrap().0.method, "index");
        let (handler, params) = t.find("get", "/tasks/42").unwrap();
        assert_eq!(handler.method, "show");
        assert_eq!(params, vec![("id".to_string(), "42".to_string())]);
        assert_eq!(t.find("POST", "/ping").unwrap().0.method, "ping");
        assert!(t.find("POST", "/tasks").is_none());
        assert!(t.add("GET", "/tasks", h("dup")).is_err());
    }
}
