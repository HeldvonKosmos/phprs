//! The route table a worker fills while running `routes/*.grd`.
//! Path syntax follows matchit: `/tasks/{id}`, `/files/{*path}`.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
    pub class: String,
    pub method: String,
}

#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub method: String,
    pub path: String,
    pub handler: Handler,
    pub middleware: Vec<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Group {
    pub prefix: String,
    pub middleware: Vec<String>,
}

#[derive(Default)]
pub struct RouteTable {
    by_method: HashMap<String, matchit::Router<usize>>,
    entries: Vec<RouteEntry>,
    /// Open `Route::group(...)` blocks, innermost last.
    pub groups: Vec<Group>,
    /// `Route::aliasMiddleware('auth', AuthMiddleware::class)`.
    pub aliases: HashMap<String, String>,
}

impl RouteTable {
    /// Adds a route under the open groups; returns its index.
    pub fn add(&mut self, method: &str, path: &str, handler: Handler) -> Result<usize, String> {
        let method = method.to_ascii_uppercase();
        let mut full = String::new();
        let mut middleware = Vec::new();
        for g in &self.groups {
            full.push_str(&normalize(&g.prefix));
            middleware.extend(g.middleware.iter().cloned());
        }
        full.push_str(&normalize(path));
        let path = normalize(&full.replace("//", "/"));
        let index = self.entries.len();
        self.by_method
            .entry(method.clone())
            .or_default()
            .insert(path.clone(), index)
            .map_err(|e| format!("route {method} {path}: {e}"))?;
        self.entries.push(RouteEntry { method, path, handler, middleware, name: None });
        Ok(index)
    }

    /// Specific method first, then `ANY`; HEAD falls back to GET.
    pub fn find(&self, method: &str, path: &str) -> Option<(RouteEntry, Vec<(String, String)>)> {
        let path = normalize(path);
        let method = method.to_ascii_uppercase();
        let method = if method == "HEAD" { "GET".to_string() } else { method };
        for m in [method.as_str(), "ANY"] {
            if let Some(router) = self.by_method.get(m)
                && let Ok(found) = router.at(&path)
            {
                let params = found.params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                return Some((self.entries[*found.value].clone(), params));
            }
        }
        None
    }

    pub fn entry_mut(&mut self, index: usize) -> Option<&mut RouteEntry> {
        self.entries.get_mut(index)
    }

    /// Middleware names → class names through the alias map.
    pub fn resolve_middleware(&self, names: &[String]) -> Vec<String> {
        // `throttle:5,1`: the alias is the part before the colon, the
        // parameters go along to `handle($request, $next, '5', '1')`
        names
            .iter()
            .map(|n| {
                let (name, params) = n.split_once(':').map_or((n.as_str(), None), |(a, b)| (a, Some(b)));
                let class = self.aliases.get(name).cloned().unwrap_or_else(|| name.to_string());
                params.map_or(class.clone(), |p| format!("{class}:{p}"))
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[RouteEntry] {
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

    fn h(m: &str) -> Handler {
        Handler { class: "C".into(), method: m.into() }
    }

    #[test]
    fn matches_params_and_any() {
        let mut t = RouteTable::default();
        t.add("get", "/tasks", h("index")).unwrap();
        t.add("GET", "/tasks/{id}", h("show")).unwrap();
        t.add("any", "/ping", h("ping")).unwrap();
        assert_eq!(t.find("GET", "/tasks/").unwrap().0.handler.method, "index");
        let (entry, params) = t.find("get", "/tasks/42").unwrap();
        assert_eq!(entry.handler.method, "show");
        assert_eq!(params, vec![("id".to_string(), "42".to_string())]);
        assert_eq!(t.find("POST", "/ping").unwrap().0.handler.method, "ping");
        assert!(t.find("POST", "/tasks").is_none());
        assert!(t.add("GET", "/tasks", h("dup")).is_err());
    }

    #[test]
    fn groups_prefix_and_middleware() {
        let mut t = RouteTable::default();
        t.aliases.insert("auth".into(), "App\\Auth".into());
        t.aliases.insert("throttle".into(), "App\\Throttle".into());
        assert_eq!(t.resolve_middleware(&["throttle:5,1".into(), "auth".into(), "X:a".into()]), vec!["App\\Throttle:5,1", "App\\Auth", "X:a"]);
        t.groups.push(Group { prefix: "/admin/".into(), middleware: vec!["auth".into()] });
        t.groups.push(Group { prefix: "tasks".into(), middleware: vec![] });
        let i = t.add("GET", "/", h("index")).unwrap();
        t.entry_mut(i).unwrap().middleware.push("Own".into());
        t.groups.clear();
        let (entry, _) = t.find("GET", "/admin/tasks").unwrap();
        assert_eq!(entry.path, "/admin/tasks");
        assert_eq!(t.resolve_middleware(&entry.middleware), vec!["App\\Auth".to_string(), "Own".to_string()]);
    }
}
