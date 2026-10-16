//! The frontend's built files, embedded in the binary at build time, and how
//! they are served: a file by its path, a page by its name (`/admin` is
//! `admin.html`), and the main page for any other path without an extension.

use axum::body::Body;
use axum::http::{HeaderValue, Response, StatusCode, Uri, header};
use include_dir::{Dir, include_dir};

static WEB: Dir<'static> = include_dir!("$OUT_DIR/web");

/// The content type of a file, by its extension.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
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

/// The embedded file a path asks for, and its name.
fn find(path: &str) -> Option<(&'static [u8], String)> {
    let path = path.trim_start_matches('/');
    let name = if path.is_empty() {
        "index.html".to_string()
    } else if path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
    {
        path.to_string()
    } else if WEB.get_file(format!("{path}.html")).is_some() {
        format!("{path}.html")
    } else {
        "index.html".to_string()
    };
    WEB.get_file(&name).map(|file| (file.contents(), name))
}

/// Serves an embedded file. A page is always asked for again; a built
/// asset's name carries its hash, so it is kept for a year.
pub async fn serve(uri: Uri) -> Response<Body> {
    let Some((bytes, name)) = find(uri.path()) else {
        // The API's error layer writes the JSON answer.
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("a fixed answer is valid");
    };
    let cache = if name.ends_with(".html") {
        "no-cache"
    } else if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&name)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}
