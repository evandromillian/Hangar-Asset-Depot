//! Browser-facing hardening, as one layer around every route (and the static
//! web client):
//!
//! - **CSRF**: state-changing `/api/` requests must come from the site itself.
//!   Browsers say where a request comes from (`Sec-Fetch-Site`, else `Origin`);
//!   the CLI and editor plugins send neither and use bearer tokens, which a
//!   browser never attaches on its own.
//! - Session tokens are not accepted in the URL (sa-token would read
//!   `?hangar_token=`): URLs end up in logs, history and `Referer`.
//! - Security headers on every response, a strict Content-Security-Policy for
//!   the web client (its inline Trunk bootstrap allowed by hash) and a sandbox
//!   CSP for API responses, so a file served from `/api/` can never run script.

use base64::Engine;
use sha2::{Digest, Sha256};
use summer_web::axum::body::{Body, to_bytes};
use summer_web::axum::extract::{Request, State};
use summer_web::axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use summer_web::axum::middleware::Next;
use summer_web::axum::response::{IntoResponse, Response};

/// Anything served under `/api/` is data, never a document.
const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'; sandbox";

/// The web client: wasm needs `'wasm-unsafe-eval'`; Leptos sets inline styles.
const PAGE_CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     media-src 'self' blob:; connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; \
     form-action 'self'; frame-ancestors 'none'";

/// Largest HTML page whose inline scripts are hashed (the SPA's index.html is a few KB).
const MAX_PAGE: usize = 1 << 20;

#[derive(Clone)]
pub struct Security {
    /// Send `Strict-Transport-Security` (the site is served over HTTPS).
    pub hsts: bool,
}

pub async fn layer(State(sec): State<Security>, req: Request, next: Next) -> Response {
    let api = req.uri().path().starts_with("/api/");
    if api && !is_safe(req.method()) && !same_origin(req.headers()) {
        tracing::warn!(method = %req.method(), path = %req.uri().path(), "cross-site request blocked");
        return (StatusCode::FORBIDDEN, "requisição de outro site bloqueada").into_response();
    }
    if api && has_query_token(req.uri().query()) {
        return (StatusCode::BAD_REQUEST, "envie o token no cabeçalho Authorization, não na URL").into_response();
    }

    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let set = |h: &mut HeaderMap, name: HeaderName, value: &'static str| {
        h.insert(name, HeaderValue::from_static(value));
    };
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::X_FRAME_OPTIONS, "DENY");
    set(h, header::REFERRER_POLICY, "same-origin");
    set(h, HeaderName::from_static("cross-origin-opener-policy"), "same-origin");
    set(h, HeaderName::from_static("cross-origin-resource-policy"), "same-origin");
    set(h, HeaderName::from_static("permissions-policy"), "camera=(), microphone=(), geolocation=(), payment=(), usb=()");
    if sec.hsts {
        set(h, header::STRICT_TRANSPORT_SECURITY, "max-age=31536000");
    }
    if api {
        set(h, header::CONTENT_SECURITY_POLICY, API_CSP);
        res
    } else if is_html(&res) {
        with_page_csp(res).await
    } else {
        res
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether a request was sent by this site's own pages (or not by a browser).
fn same_origin(h: &HeaderMap) -> bool {
    let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
    // Every current browser sends it; "none" = typed by the user, not a page.
    if let Some(site) = get("sec-fetch-site") {
        return matches!(site, "same-origin" | "none");
    }
    // Older browsers: the Origin's host must be the one the request was sent to.
    match get("origin") {
        None => true,
        Some(origin) => {
            let host = get("x-forwarded-host").or_else(|| get("host"));
            origin.split_once("://").is_some_and(|(_, o)| Some(o) == host)
        }
    }
}

fn has_query_token(query: Option<&str>) -> bool {
    query.is_some_and(|q| q.split('&').any(|kv| kv.split('=').next() == Some(crate::auth::COOKIE_NAME)))
}

fn is_html(res: &Response) -> bool {
    res.status() == StatusCode::OK
        && !res.headers().contains_key(header::CONTENT_ENCODING)
        && res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/html"))
}

/// Adds the page CSP, allowing the page's own inline scripts by hash.
async fn with_page_csp(res: Response) -> Response {
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_PAGE).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let hashes: String = inline_scripts(&String::from_utf8_lossy(&bytes))
        .map(|s| format!(" 'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(s))))
        .collect();
    let csp = format!("{PAGE_CSP}; script-src 'self' 'wasm-unsafe-eval'{hashes}");
    parts.headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&csp).expect("ASCII CSP"));
    Response::from_parts(parts, Body::from(bytes))
}

/// Contents of the `<script>` elements without `src` (what CSP hashes).
fn inline_scripts(html: &str) -> impl Iterator<Item = &str> {
    html.split("<script").skip(1).filter_map(|rest| {
        let (attrs, rest) = rest.split_once('>')?;
        let (body, _) = rest.split_once("</script>")?;
        (!attrs.contains("src=")).then_some(body)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs.iter().map(|(k, v)| (HeaderName::from_static(k), HeaderValue::from_static(v))).collect()
    }

    #[test]
    fn cross_site_requests() {
        assert!(same_origin(&headers(&[])), "CLI / editor plugins");
        assert!(same_origin(&headers(&[("sec-fetch-site", "same-origin")])));
        assert!(!same_origin(&headers(&[("sec-fetch-site", "cross-site")])));
        assert!(!same_origin(&headers(&[("sec-fetch-site", "same-site")])), "sibling subdomains too");
        assert!(same_origin(&headers(&[("origin", "https://hangar.example"), ("host", "hangar.example")])));
        assert!(!same_origin(&headers(&[("origin", "https://evil.example"), ("host", "hangar.example")])));
        assert!(!same_origin(&headers(&[("origin", "null"), ("host", "hangar.example")])));
    }

    #[test]
    fn query_tokens() {
        assert!(has_query_token(Some("a=1&hangar_token=abc")));
        assert!(!has_query_token(Some("download=1")));
        assert!(!has_query_token(None));
    }

    #[test]
    fn finds_inline_scripts() {
        let html = r#"<script type="module">import init from '/a.js';</script><script src="/b.js"></script><script>x()</script>"#;
        assert_eq!(inline_scripts(html).collect::<Vec<_>>(), ["import init from '/a.js';", "x()"]);
    }
}
