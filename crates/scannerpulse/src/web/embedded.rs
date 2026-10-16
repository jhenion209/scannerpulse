//! The frontend's built files, embedded in the binary at build time, and how
//! they are served: a file by its path, a page by its name (`/admin` is
//! `admin.html`), and the main page for any other path without an extension.

use axum::body::Body;
use axum::http::{HeaderValue, Method, Response, StatusCode, Uri, header};
use include_dir::{Dir, include_dir};

static WEB: Dir<'static> = include_dir!("$OUT_DIR/web");

/// The file a path asks for: a file by its own path, a page by its name, the
/// main page for any other path without an extension. `has` says whether the
/// build holds a file of that name.
fn resolve(path: &str, has: impl Fn(&str) -> bool) -> Option<String> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return has("index.html").then(|| "index.html".to_string());
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    if last.contains('.') {
        return has(path).then(|| path.to_string());
    }
    let page = format!("{path}.html");
    if has(&page) {
        Some(page)
    } else if has("index.html") {
        Some("index.html".to_string())
    } else {
        None
    }
}

/// The content type of a file, by its extension.
fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// A page is asked for again on every load; a built asset's name carries its
/// hash, so it is kept for a year.
fn cache_control(name: &str) -> &'static str {
    if name.ends_with(".html") {
        "no-cache"
    } else if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    }
}

/// Serves an embedded file to a GET or a HEAD; any other method is refused.
pub async fn serve(method: Method, uri: Uri) -> Response<Body> {
    if method != Method::GET && method != Method::HEAD {
        // The API's error layer writes the JSON answer.
        return Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ALLOW, "GET, HEAD")
            .body(Body::empty())
            .expect("a fixed answer is valid");
    }
    let found = resolve(uri.path(), |name| WEB.get_file(name).is_some())
        .and_then(|name| WEB.get_file(&name).map(|file| (file.contents(), name)));
    let Some((bytes, name)) = found else {
        // The API's error layer writes the JSON answer.
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("a fixed answer is valid");
    };
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&name)),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control(&name)),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(name: &str) -> bool {
        ["index.html", "admin.html", "assets/app-1.js"].contains(&name)
    }

    #[test]
    fn a_path_finds_its_file_its_page_or_the_main_page() {
        assert_eq!(resolve("/", built).as_deref(), Some("index.html"));
        assert_eq!(resolve("/admin", built).as_deref(), Some("admin.html"));
        assert_eq!(resolve("/admin/", built).as_deref(), Some("admin.html"));
        assert_eq!(resolve("/calls/1", built).as_deref(), Some("index.html"));
        assert_eq!(
            resolve("/assets/app-1.js", built).as_deref(),
            Some("assets/app-1.js")
        );
        assert_eq!(resolve("/assets/missing.js", built), None);
    }

    #[test]
    fn no_page_and_no_main_page_is_not_found() {
        assert_eq!(resolve("/admin", |_| false), None);
        assert_eq!(resolve("/", |_| false), None);
    }

    #[test]
    fn each_kind_of_file_has_its_content_type() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type("assets/app-1.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("assets/app-1.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("assets/logo.bin"), "application/octet-stream");
    }

    #[test]
    fn pages_are_revalidated_and_hashed_assets_kept_a_year() {
        assert_eq!(cache_control("admin.html"), "no-cache");
        assert_eq!(
            cache_control("assets/app-1.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control("favicon.ico"), "public, max-age=3600");
    }
}
