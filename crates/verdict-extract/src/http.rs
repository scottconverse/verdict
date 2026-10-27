//! One way to write a URL path, so a route and a call that mean the same path compare equal.
//!
//! Every parameter style becomes `{}`: FastAPI `{id}`, Starlette `{id:int}`, Express `:id`,
//! TanStack `$id`, template literals `${id}`, bracket segments `[id]`, Flask `<int:id>`.

/// Normalize a path or URL as written in code. Drops scheme and host, query and fragment, empty
/// segments and the trailing slash. Returns `/` for the root.
///
/// A leading segment that is a whole template expression (`${API_BASE}/users`) is treated as the
/// base URL and dropped.
pub fn normalize_path(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(i) = s.find(['?', '#']) {
        s = &s[..i];
    }
    if let Some(rest) = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
    {
        s = match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        };
    }
    let mut out: Vec<&str> = Vec::new();
    let mut first = true;
    for seg in s.split('/') {
        if seg.is_empty() {
            first = false;
            continue;
        }
        if first && is_whole_template(seg) {
            first = false;
            continue;
        }
        first = false;
        out.push(if is_param(seg) { "{}" } else { seg });
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

/// Join a router prefix and a route path, then normalize.
pub fn join(prefix: &str, path: &str) -> String {
    normalize_path(&format!("{prefix}/{path}"))
}

/// True when two normalized paths can name the same route: equal segment counts, and each pair of
/// segments equal or one of them `{}`.
pub fn paths_match(a: &str, b: &str) -> bool {
    let a: Vec<&str> = a.split('/').filter(|s| !s.is_empty()).collect();
    let b: Vec<&str> = b.split('/').filter(|s| !s.is_empty()).collect();
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(x, y)| x == y || *x == "{}" || *y == "{}")
}

/// True when two HTTP methods can name the same route (`None` matches any).
pub fn methods_match(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
        _ => true,
    }
}

fn is_whole_template(seg: &str) -> bool {
    seg.starts_with("${") && seg.ends_with('}') && seg.matches("${").count() == 1
}

fn is_param(seg: &str) -> bool {
    seg.contains("${")
        || (seg.starts_with('{') && seg.ends_with('}'))
        || (seg.starts_with('[') && seg.ends_with(']'))
        || (seg.starts_with('<') && seg.ends_with('>'))
        || seg.starts_with(':')
        || seg.starts_with('$')
        || seg == "*"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_meet() {
        for raw in [
            "/users/{id}",
            "/users/{id:int}",
            "/users/:id",
            "/users/$id",
            "/users/${id}",
            "/users/[id]",
            "/users/<int:id>",
            "users/{user_id}/",
            "https://api.example.com/users/{id}?x=1",
            "${API_BASE}/users/${u.id}",
        ] {
            assert_eq!(normalize_path(raw), "/users/{}", "{raw}");
        }
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(join("/api/", "/items"), "/api/items");
        assert_eq!(join("", "/"), "/");
    }

    #[test]
    fn matching() {
        assert!(paths_match("/users/{}", "/users/42"));
        assert!(paths_match("/users/{}/posts", "/users/{}/posts"));
        assert!(!paths_match("/users/{}", "/users"));
        assert!(!paths_match("/users/{}", "/teams/{}"));
        assert!(methods_match(Some("GET"), Some("get")));
        assert!(methods_match(None, Some("POST")));
        assert!(!methods_match(Some("GET"), Some("POST")));
    }
}
