use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{Html, IntoResponse, Response},
};

const UI: &str = include_str!("../../../../assets/index.html");
const UI_JS: &[u8] = include_bytes!("../../../../assets/app.js");
const UI_JS_GZIP: &[u8] = include_bytes!("../../../../assets/app.js.gz");
const UI_JS_HASH: &str = include_str!("../../../../assets/app.js.sha256");
const UI_CSS: &[u8] = include_bytes!("../../../../assets/app.css");
const UI_CSS_GZIP: &[u8] = include_bytes!("../../../../assets/app.css.gz");
const UI_CSS_HASH: &str = include_str!("../../../../assets/app.css.sha256");
const UI_CHART: &[u8] = include_bytes!("../../../../assets/chart.js");
const UI_CHART_GZIP: &[u8] = include_bytes!("../../../../assets/chart.js.gz");
const UI_CHART_HASH: &str = include_str!("../../../../assets/chart.js.sha256");

pub(super) async fn index() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Html(UI))
}

pub(super) async fn javascript(uri: Uri, headers: HeaderMap) -> Response {
    asset_response(
        &uri,
        &headers,
        "application/javascript; charset=utf-8",
        UI_JS_HASH,
        UI_JS,
        UI_JS_GZIP,
    )
}

pub(super) async fn stylesheet(uri: Uri, headers: HeaderMap) -> Response {
    asset_response(
        &uri,
        &headers,
        "text/css; charset=utf-8",
        UI_CSS_HASH,
        UI_CSS,
        UI_CSS_GZIP,
    )
}

pub(super) async fn chart(uri: Uri, headers: HeaderMap) -> Response {
    asset_response(
        &uri,
        &headers,
        "application/javascript; charset=utf-8",
        UI_CHART_HASH,
        UI_CHART,
        UI_CHART_GZIP,
    )
}

fn asset_response(
    uri: &Uri,
    headers: &HeaderMap,
    content_type: &'static str,
    fingerprint: &str,
    identity: &'static [u8],
    gzip: &'static [u8],
) -> Response {
    let requested_fingerprint = uri.query().and_then(|query| query.strip_prefix("v="));
    let stale = requested_fingerprint.is_some_and(|requested| requested != fingerprint);
    let encoding = preferred_encoding(headers);
    let compressed = encoding == Some(AssetEncoding::Gzip);
    let mut response = Response::new(if stale || encoding.is_none() {
        Body::empty()
    } else {
        Body::from(if compressed { gzip } else { identity })
    });
    *response.status_mut() = if stale {
        StatusCode::NOT_FOUND
    } else if encoding.is_none() {
        StatusCode::NOT_ACCEPTABLE
    } else {
        StatusCode::OK
    };
    let response_headers = response.headers_mut();
    response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(
            if requested_fingerprint == Some(fingerprint) && encoding.is_some() {
                "public, max-age=31536000, immutable"
            } else {
                "no-store"
            },
        ),
    );
    response_headers.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if !stale && encoding.is_some() && compressed {
        response_headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    response
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AssetEncoding {
    Identity,
    Gzip,
}

fn preferred_encoding(headers: &HeaderMap) -> Option<AssetEncoding> {
    if !headers.contains_key(header::ACCEPT_ENCODING) {
        return Some(AssetEncoding::Identity);
    }
    let mut gzip_quality: Option<u16> = None;
    let mut identity_quality: Option<u16> = None;
    let mut wildcard_quality: Option<u16> = None;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for coding in value.split(',') {
            let mut parts = coding.split(';');
            let encoding = parts.next().unwrap_or_default().trim();
            let mut quality = 1000;
            for parameter in parts {
                let Some((name, value)) = parameter.split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("q") {
                    quality = parse_quality(value.trim()).unwrap_or(0);
                }
            }
            if encoding.eq_ignore_ascii_case("gzip") {
                gzip_quality = Some(gzip_quality.unwrap_or(0).max(quality));
            } else if encoding.eq_ignore_ascii_case("identity") {
                identity_quality = Some(identity_quality.unwrap_or(0).max(quality));
            } else if encoding == "*" {
                wildcard_quality = Some(wildcard_quality.unwrap_or(0).max(quality));
            }
        }
    }
    let gzip_quality = gzip_quality.or(wildcard_quality).unwrap_or(0);
    let identity_quality =
        identity_quality.unwrap_or_else(|| if wildcard_quality == Some(0) { 0 } else { 1000 });
    if gzip_quality == 0 && identity_quality == 0 {
        None
    } else if gzip_quality >= identity_quality {
        Some(AssetEncoding::Gzip)
    } else {
        Some(AssetEncoding::Identity)
    }
}

fn parse_quality(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match whole {
        "0" => Some(fraction.parse().unwrap_or(0) * 10_u16.pow(3 - fraction.len() as u32)),
        "1" if fraction.bytes().all(|byte| byte == b'0') => Some(1000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_versioned_compressed_assets_with_safe_caching() {
        let uri = "/app.js?v=current".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip"),
        );
        let response = asset_response(
            &uri,
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");

        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0, *;q=1"),
        );
        let response = asset_response(
            &"/app.js?v=old".parse().unwrap(),
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");

        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0.1, identity;q=1"),
        );
        let response = asset_response(
            &uri,
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));

        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0, identity;q=0"),
        );
        let response = asset_response(
            &uri,
            &headers,
            "application/javascript",
            "current",
            b"identity",
            b"gzip",
        );
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
