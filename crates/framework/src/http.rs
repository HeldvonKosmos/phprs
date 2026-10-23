//! Plain request/response records that cross the channel between Tokio and
//! the worker threads, plus the two conversions at the Axum edge.

/// A request as the worker sees it: everything already read, no futures.
#[derive(Debug, Clone, Default)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn get(path: &str) -> Self {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        HttpRequest { method: "GET".into(), path: p.into(), query: q.into(), ..Default::default() }
    }

    pub fn post(path: &str, content_type: &str, body: &[u8]) -> Self {
        HttpRequest {
            method: "POST".into(),
            path: path.into(),
            headers: vec![("content-type".into(), content_type.into())],
            body: body.to_vec(),
            ..Default::default()
        }
    }

    /// Header value, case-insensitive on the name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn query_pairs(&self) -> Vec<(String, String)> {
        parse_query(&self.query)
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn with_type(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        HttpResponse { status, headers: vec![("content-type".into(), content_type.into())], body }
    }
    pub fn json(status: u16, body: Vec<u8>) -> Self {
        Self::with_type(status, "application/json", body)
    }
    pub fn html(status: u16, body: Vec<u8>) -> Self {
        Self::with_type(status, "text/html; charset=utf-8", body)
    }
    pub fn text(status: u16, body: &str) -> Self {
        Self::with_type(status, "text/plain; charset=utf-8", body.as_bytes().to_vec())
    }
    pub fn empty(status: u16) -> Self {
        HttpResponse { status, headers: Vec::new(), body: Vec::new() }
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// `a=1&b=x%20y&c` → pairs, percent-decoded, `+` as space.
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const MAX_BODY: usize = 16 * 1024 * 1024;

/// Read an Axum request completely (bounded body) into an `HttpRequest`.
pub async fn from_axum(req: axum::extract::Request) -> Result<HttpRequest, HttpResponse> {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, MAX_BODY).await.map_err(|_| HttpResponse::text(413, "payload too large"))?;
    let headers = parts
        .headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    Ok(HttpRequest {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or("").to_string(),
        headers,
        body: bytes.to_vec(),
    })
}

pub fn into_axum(r: HttpResponse) -> axum::response::Response {
    let mut b = axum::http::Response::builder().status(r.status);
    for (k, v) in &r.headers {
        b = b.header(k, v);
    }
    b.body(axum::body::Body::from(r.body)).unwrap_or_else(|e| {
        axum::http::Response::builder().status(500).body(axum::body::Body::from(format!("invalid response: {e}"))).expect("static response")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_parsing() {
        assert_eq!(super::parse_query("a=1&b=x%20y+z&c"), vec![("a".into(), "1".into()), ("b".into(), "x y z".into()), ("c".into(), String::new())]);
        assert_eq!(super::percent_decode("100%"), "100%");
    }
}
