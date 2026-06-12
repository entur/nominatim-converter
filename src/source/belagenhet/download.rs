use crate::common::input::{
    CacheOptions, DownloadStream, ResolvedInput, USER_AGENT, fetch_and_resolve, is_cached,
};

const BASE_URL: &str = "https://dl1.lantmateriet.se/adress/belagenhetsadresser";

/// Download belägenhetsadresser GeoPackage for a given municipality and
/// extract the `.gpkg` from its ZIP.
///
/// Credentials are read from env vars `LANTMATERIET_USER` / `LANTMATERIET_PASS`
/// (also loaded from `.env` via dotenvy). With a warm cache, credentials
/// aren't needed -- the ZIP is reused from disk.
///
/// Returns a `ResolvedInput` whose path points at the extracted `.gpkg`.
/// Temp files are cleaned up on drop; cached files are preserved.
pub fn download_municipality(
    kommun_id: &str,
    cache: &CacheOptions,
) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    let url = municipality_url(kommun_id);
    // Fail fast on missing credentials, but only when the download will
    // actually hit the network -- a warm cache run needs no auth, and a
    // missing env var would otherwise be retried as if it were transient.
    if cache.is_refresh() || !is_cached(&url, cache) {
        load_credentials()?;
    }
    fetch_and_resolve(&url, Some("*.gpkg"), cache, fetch_with_basic_auth)
        .map_err(describe_download_error)
}

/// Turn raw HTTP errors into operator-friendly messages: 401/403 get a
/// credentials hint, other HTTP errors get Lantmäteriet attribution, and
/// non-HTTP errors (extraction, disk) pass through unchanged.
///
/// This runs *after* the retry layer in `fetch_and_resolve`, which needs the
/// raw `ureq::Error` to tell transient failures from permanent ones -- so the
/// error must stay unwrapped until this point.
fn describe_download_error(e: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
    match e.downcast_ref::<ureq::Error>() {
        // ureq 3.x returns Err for 4xx/5xx, so auth failures arrive here.
        Some(ureq::Error::StatusCode(401 | 403)) => {
            format!("Authentication failed. Check LANTMATERIET_USER and LANTMATERIET_PASS. ({e})")
                .into()
        }
        Some(_) => format!("Failed to download from Lantmäteriet: {e}").into(),
        None => e,
    }
}

/// URL for a single municipality's belägenhetsadresser archive.
pub(crate) fn municipality_url(kommun_id: &str) -> String {
    format!("{BASE_URL}/belagenhetsadresser_kn{kommun_id}.zip")
}

fn fetch_with_basic_auth(url: &str) -> Result<DownloadStream, Box<dyn std::error::Error>> {
    let (user, pass) = load_credentials()?;
    let encoded = base64_encode(format!("{user}:{pass}").as_bytes());

    // Errors are returned raw (not stringified) so the retry layer in
    // `fetch_and_resolve` can tell transient failures from permanent ones.
    let response = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Authorization", &format!("Basic {encoded}"))
        .call()?;

    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    Ok(DownloadStream::new(
        Box::new(response.into_body().into_reader()),
        content_length,
    ))
}

fn load_credentials() -> Result<(String, String), Box<dyn std::error::Error>> {
    let user = std::env::var("LANTMATERIET_USER")
        .map_err(|_| "LANTMATERIET_USER environment variable not set. Set it directly or in a .env file.")?;
    let pass = std::env::var("LANTMATERIET_PASS")
        .map_err(|_| "LANTMATERIET_PASS environment variable not set. Set it directly or in a .env file.")?;

    Ok((user, pass))
}

/// Hand-rolled base64 (RFC 4648, no line wrapping) to avoid pulling in a
/// direct dependency for a single Basic-auth header.
fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe_download_error_adds_credentials_hint_for_auth_failures() {
        for code in [401u16, 403] {
            let err: Box<dyn std::error::Error> = Box::new(ureq::Error::StatusCode(code));
            let msg = describe_download_error(err).to_string();
            assert!(msg.contains("LANTMATERIET_USER"), "HTTP {code}: got '{msg}'");
        }
    }

    #[test]
    fn test_describe_download_error_attributes_other_http_errors() {
        let err: Box<dyn std::error::Error> = Box::new(ureq::Error::StatusCode(500));
        let msg = describe_download_error(err).to_string();
        assert!(msg.contains("Lantmäteriet"), "got '{msg}'");
        assert!(!msg.contains("LANTMATERIET_USER"));
    }

    #[test]
    fn test_describe_download_error_passes_non_http_errors_through() {
        let err: Box<dyn std::error::Error> = "disk full".into();
        assert_eq!(describe_download_error(err).to_string(), "disk full");
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }
}
