use crate::common::util::fnv1a_64;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Name of the environment variable consulted by the CLI when `-d/--cache-dir`
/// isn't provided. Exposed so `main.rs` and the clap `env = …` attribute
/// reference the same string.
pub const CACHE_DIR_ENV: &str = "NOMINATIM_CACHE_DIR";

/// User-Agent sent on every outbound HTTP request. Some upstreams (Geonorge,
/// Lantmäteriet, SCB) expect a self-identifying agent.
pub const USER_AGENT: &str = concat!("nominatim-converter/", env!("CARGO_PKG_VERSION"));

/// Controls the download cache. Plumbed explicitly through `resolve_input` and
/// `fetch_and_resolve` -- there is no global state, no env-var back-channel.
///
/// Construct with `CacheOptions::default()` (no cache) or
/// `CacheOptions::new(dir, refresh)`. An empty `dir` is treated as unset so a
/// cleared env var (`NOMINATIM_CACHE_DIR=`) doesn't land files at filesystem
/// root.
///
/// Owns its `PathBuf` so callee signatures don't need a lifetime parameter.
/// Cloning is cheap (one `PathBuf` alloc) relative to the download cost.
#[derive(Clone, Default)]
pub struct CacheOptions {
    dir: Option<PathBuf>,
    refresh: bool,
}

impl CacheOptions {
    /// Cache downloads under `dir`. An empty path counts as no cache.
    /// If `refresh` is true, existing cache entries are ignored and overwritten
    /// with a fresh download (useful for rolling URLs like `Current_latest.zip`).
    /// For "no cache", use `CacheOptions::default()`.
    pub fn new(dir: Option<&Path>, refresh: bool) -> Self {
        let dir = dir.filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf);
        Self { dir, refresh }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn is_refresh(&self) -> bool {
        self.refresh
    }
}

/// A resolved input file ready to be consumed by a source converter.
///
/// When the file was downloaded to a temp location (no cache, or an
/// extracted-from-ZIP output without cache), it's removed on drop. Cached
/// files are preserved.
pub struct ResolvedInput {
    path: PathBuf,
    is_temp: bool,
}

impl ResolvedInput {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn temp(path: PathBuf) -> Self {
        Self { path, is_temp: true }
    }

    fn persistent(path: PathBuf) -> Self {
        Self { path, is_temp: false }
    }
}

impl Drop for ResolvedInput {
    fn drop(&mut self) {
        if self.is_temp {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

/// A streaming HTTP response body plus its advertised `Content-Length`, if any.
/// Callers of `fetch_and_resolve` return this from their request closure;
/// `default_fetch` produces one via a plain `ureq::get`.
///
/// Marked `#[non_exhaustive]` so future needs (ETag, Last-Modified, etc. for
/// conditional-GET support against rolling URLs) can be added without a
/// breaking change.
#[non_exhaustive]
pub struct DownloadStream {
    pub reader: Box<dyn Read>,
    pub content_length: Option<u64>,
}

impl DownloadStream {
    pub fn new(reader: Box<dyn Read>, content_length: Option<u64>) -> Self {
        Self { reader, content_length }
    }
}

/// Return `true` when `cache.dir` is set and a cache entry already exists for
/// `url` (either the raw download or its extracted output). Callers can use
/// this to decide whether the next `resolve_input` call will hit the network.
pub fn is_cached(url: &str, cache: &CacheOptions) -> bool {
    let Some(dir) = cache.dir() else { return false };
    let (raw, extracted) = cache_paths(dir, url);
    raw.exists() || extracted.is_some_and(|p| p.exists())
}

/// Derive the cache locations for `url` under `dir`: the raw download path,
/// plus the extracted-output path when the URL points at a ZIP. Shared by
/// `is_cached` and `fetch_and_resolve` so they always agree on what counts
/// as a cache entry.
fn cache_paths(dir: &Path, url: &str) -> (PathBuf, Option<PathBuf>) {
    let parsed = parse_url(url);
    let raw = cache_path_in(dir, &parsed.normalized, &parsed.basename);
    let extracted = parsed.is_zip.then(|| append_suffix(&raw, ".extracted"));
    (raw, extracted)
}

/// Resolve an input that may be a local file or an HTTP(S) URL.
/// For URLs, downloads via a default GET request. ZIP archives are extracted
/// to the first entry matching `extract_glob` (or the first non-directory entry).
/// Local `.zip` inputs are extracted the same way (to a temp file); other local
/// files are used in place.
///
/// When `cache.dir` is set, downloads are persisted and reused on subsequent runs.
pub fn resolve_input(
    input: &Path,
    extract_glob: Option<&str>,
    cache: &CacheOptions,
) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    let input_str = input.to_string_lossy();
    if input_str.starts_with("http://") || input_str.starts_with("https://") {
        return fetch_and_resolve(input_str.as_ref(), extract_glob, cache, default_fetch);
    }
    // Local file. Auto-extract ZIP archives to a temp file (cleaned up on drop) so the CLI's
    // documented "ZIP archives are extracted automatically" holds for local inputs too, matching
    // the URL path -- without it, a local `.zip` is fed straight to a converter as raw bytes.
    // Non-ZIP files are used in place. Local extraction isn't cached: there's nothing to
    // re-download, extraction is cheap, and a path-keyed cache would risk serving a stale extract
    // after the file changes.
    if is_zip_path(input) {
        return Ok(ResolvedInput::temp(extract_from_zip(input, extract_glob)?));
    }
    Ok(ResolvedInput::persistent(input.to_path_buf()))
}

/// Whether `path` names a ZIP archive, by extension (case-insensitive). Mirrors the
/// extension-based `is_zip` decision `parse_url` makes for download URLs.
fn is_zip_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Download a URL (or load from cache), extracting ZIPs if needed.
/// `fetch` is only invoked on cache miss (or when `cache.refresh` is set);
/// this is the seam callers use to customize the request -- e.g., to add
/// `Authorization` headers for Lantmäteriet. Transient failures are retried
/// (see `is_retryable`), so `fetch` may be called up to `DOWNLOAD_ATTEMPTS`
/// times.
///
/// **Crate-internal extension point.** Public because `source::belagenhet`
/// lives in a sibling module and needs custom-auth downloads. Not a stable
/// external API; the closure signature and `DownloadStream` shape may change.
pub fn fetch_and_resolve<F>(
    url: &str,
    extract_glob: Option<&str>,
    cache: &CacheOptions,
    fetch: F,
) -> Result<ResolvedInput, Box<dyn std::error::Error>>
where
    F: Fn(&str) -> Result<DownloadStream, Box<dyn std::error::Error>>,
{
    let parsed = parse_url(url);
    let (raw_cache, extracted_cache) = match cache.dir() {
        Some(dir) => {
            let (raw, extracted) = cache_paths(dir, url);
            (Some(raw), extracted)
        }
        None => (None, None),
    };

    // Fast path: extracted file already cached (skip even the zip).
    if !cache.refresh
        && let Some(p) = &extracted_cache
        && p.exists()
    {
        eprintln!("Using cached extract: {}", p.display());
        return Ok(ResolvedInput::persistent(p.clone()));
    }

    // Cache hit on the raw download: re-extract if it's a zip.
    if !cache.refresh
        && let Some(p) = &raw_cache
        && p.exists()
    {
        eprintln!("Using cached download: {}", p.display());
        if parsed.is_zip {
            let dst = extracted_cache.as_ref().expect("extracted_cache set when is_zip");
            extract_from_zip_to(p, extract_glob, dst)?;
            return Ok(ResolvedInput::persistent(dst.clone()));
        }
        return Ok(ResolvedInput::persistent(p.clone()));
    }

    // Miss (or --refresh-cache): download.
    if cache.refresh
        && let Some(p) = raw_cache.as_ref().filter(|p| p.exists())
    {
        eprintln!("Refreshing cached: {} (--refresh-cache)", p.display());
    }
    let (raw_path, raw_is_temp) = match raw_cache.as_ref() {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            (p.clone(), false)
        }
        None => {
            let ext = if parsed.is_zip { "zip" } else { parsed.extension.as_str() };
            (make_temp_path(ext), true)
        }
    };

    download_with_retry(url, &raw_path, fetch)?;

    if !parsed.is_zip {
        return Ok(if raw_is_temp {
            ResolvedInput::temp(raw_path)
        } else {
            ResolvedInput::persistent(raw_path)
        });
    }

    // Zip: extract to cache (if caching) or to temp.
    let extracted = match extracted_cache.as_ref() {
        Some(dst) => {
            extract_from_zip_to(&raw_path, extract_glob, dst)?;
            ResolvedInput::persistent(dst.clone())
        }
        None => ResolvedInput::temp(extract_from_zip(&raw_path, extract_glob)?),
    };
    if raw_is_temp {
        std::fs::remove_file(&raw_path).ok();
    }
    Ok(extracted)
}

/// Total attempts per download (initial try plus retries).
const DOWNLOAD_ATTEMPTS: u32 = 3;

/// Delay before the first retry; doubles after each subsequent failure.
/// Zero under `cfg(test)` so the retry unit tests don't sleep for real.
#[cfg(not(test))]
const RETRY_BASE_DELAY_SECS: u64 = 2;
#[cfg(test)]
const RETRY_BASE_DELAY_SECS: u64 = 0;

/// Fetch `url` and stream it to `path`, retrying transient failures.
/// A failure anywhere -- the request itself or mid-stream while writing --
/// restarts the whole download. The partial file is removed before each retry
/// (and on final failure) so a truncated download is never mistaken for a
/// complete cache entry on the next run.
fn download_with_retry(
    url: &str,
    path: &Path,
    fetch: impl Fn(&str) -> Result<DownloadStream, Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut attempt = 1;
    loop {
        eprintln!("Downloading {url}...");
        let result = fetch(url)
            .and_then(|stream| download_to_file(stream.reader, path, stream.content_length));
        let Err(err) = result else { return Ok(()) };
        std::fs::remove_file(path).ok();
        if attempt >= DOWNLOAD_ATTEMPTS || !is_retryable(err.as_ref()) {
            eprintln!("Download failed (attempt {attempt}/{DOWNLOAD_ATTEMPTS}), giving up: {err}");
            return Err(err);
        }
        let delay = RETRY_BASE_DELAY_SECS * 2u64.pow(attempt - 1);
        eprintln!("Download failed (attempt {attempt}/{DOWNLOAD_ATTEMPTS}): {err}");
        eprintln!("Retrying in {delay}s...");
        std::thread::sleep(std::time::Duration::from_secs(delay));
        attempt += 1;
    }
}

/// Whether a failed download is worth retrying. HTTP 4xx (except 408/429)
/// means the request itself is wrong -- bad URL, bad credentials -- and will
/// fail identically on retry. Everything else (connection failures, timeouts,
/// 5xx, mid-stream I/O errors) is treated as transient.
fn is_retryable(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(e) = current {
        if let Some(ureq_err) = e.downcast_ref::<ureq::Error>() {
            return match ureq_err {
                ureq::Error::StatusCode(code) => *code >= 500 || *code == 408 || *code == 429,
                _ => true,
            };
        }
        // io::Error::source() skips the error it wraps (it returns the inner
        // error's own source), so step into the wrapped error explicitly.
        current = if let Some(io_err) = e.downcast_ref::<io::Error>() {
            io_err.get_ref().map(|inner| inner as &(dyn std::error::Error + 'static))
        } else {
            e.source()
        };
    }
    true
}

fn default_fetch(url: &str) -> Result<DownloadStream, Box<dyn std::error::Error>> {
    let response = ureq::get(url).header("User-Agent", USER_AGENT).call()?;
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

/// Parsed view of an input URL. `normalized` is used for cache-key hashing --
/// fragment-stripped, scheme+authority lowercased, query string preserved.
/// `basename`/`extension`/`is_zip` are derived from the path component only so
/// query strings like `?token=…` don't pollute cache filenames.
struct ParsedUrl {
    normalized: String,
    basename: String,
    extension: String,
    is_zip: bool,
}

fn parse_url(url: &str) -> ParsedUrl {
    // 1. Strip fragment (not cache-significant).
    let without_frag = url.split_once('#').map_or(url, |(head, _)| head);

    // 2. Split scheme://authority/... from the rest; lowercase scheme+authority.
    let (normalized, path_and_query) = match without_frag.split_once("://") {
        Some((scheme, rest)) => {
            let scheme_lower = scheme.to_ascii_lowercase();
            let (authority, path_and_query) = rest.split_once('/').unwrap_or((rest, ""));
            let authority_lower = authority.to_ascii_lowercase();
            let normalized = if path_and_query.is_empty() {
                format!("{scheme_lower}://{authority_lower}")
            } else {
                format!("{scheme_lower}://{authority_lower}/{path_and_query}")
            };
            (normalized, path_and_query)
        }
        None => (without_frag.to_string(), without_frag),
    };

    // 3. Basename/extension come from the path (no query) only.
    let path_only = path_and_query.split_once('?').map_or(path_and_query, |(p, _)| p);
    let basename = Path::new(path_only)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string();
    let extension = Path::new(&basename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string();
    let is_zip = extension.eq_ignore_ascii_case("zip");

    ParsedUrl { normalized, basename, extension, is_zip }
}

/// Compute the cache file path for a (normalized) URL and its basename.
/// Uses FNV-1a of the normalized URL as a collision-avoiding prefix; the
/// basename is kept verbatim so `file(1)` and casual directory listings stay
/// informative. Filenames are stable across Rust compiler upgrades.
fn cache_path_in(dir: &Path, normalized_url: &str, basename: &str) -> PathBuf {
    let hash = fnv1a_64(normalized_url.as_bytes());
    dir.join(format!("{hash:016x}-{basename}"))
}

/// Append an extra suffix to a path, preserving the original (e.g. `.zip`).
/// `{dir}/{hash}-file.zip` becomes `{dir}/{hash}-file.zip.extracted`.
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

pub(crate) fn make_temp_path(ext: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir();
    let id = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    if ext.is_empty() {
        dir.join(format!("nominatim-converter-{id}-{ts}-{seq}.tmp"))
    } else {
        dir.join(format!("nominatim-converter-{id}-{ts}-{seq}.{ext}"))
    }
}

pub(crate) fn download_to_file(
    mut reader: impl Read,
    path: &Path,
    content_length: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::create(path)?;
    let mut downloaded: u64 = 0;
    let mut last_report: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];

    let result = loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(e),
        };
        if let Err(e) = file.write_all(&buf[..n]) {
            break Err(e);
        }
        downloaded += n as u64;

        if downloaded - last_report >= 50_000_000 {
            if let Some(total) = content_length {
                let pct = (downloaded as f64 / total as f64 * 100.0) as u64;
                eprint!("\r  {:.0} MB / {:.0} MB ({pct}%)", downloaded as f64 / 1e6, total as f64 / 1e6);
            } else {
                eprint!("\r  {:.0} MB downloaded", downloaded as f64 / 1e6);
            }
            last_report = downloaded;
        }
    };

    // Terminate the `\r` progress line even when the copy failed mid-stream,
    // so the error/retry message that follows starts on its own line.
    if last_report > 0 {
        eprintln!();
    }
    result?;

    let size_mb = downloaded as f64 / (1024.0 * 1024.0);
    eprintln!("Downloaded {size_mb:.1} MB to {}", path.display());
    Ok(())
}

/// Extract the first matching entry from `zip_path` to a new temp file.
pub(crate) fn extract_from_zip(
    zip_path: &Path,
    glob_pattern: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let (entry_name, _) = find_zip_entry(zip_path, glob_pattern)?;
    let ext = Path::new(&entry_name)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let out_path = make_temp_path(&ext);
    extract_from_zip_to(zip_path, glob_pattern, &out_path)?;
    Ok(out_path)
}

/// Extract the first matching entry from `zip_path` to `out_path`.
/// `out_path`'s parent directory is created if needed.
fn extract_from_zip_to(
    zip_path: &Path,
    glob_pattern: Option<&str>,
    out_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let (entry_name, index) = find_zip_entry(zip_path, glob_pattern)?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = File::open(zip_path)?;
    let reader = BufReader::new(file);
    let mut archive = zip::ZipArchive::new(reader)?;
    let mut entry = archive.by_index(index)?;
    let expected = entry.size();

    // Extract to a temp sibling, then rename into place. Writing straight to `out_path`
    // would leave a truncated file at the canonical cache path if the process is killed
    // (SIGTERM/crash/OOM) or the disk fills mid-copy - and the cache trusts entries by
    // existence alone, so every later run would silently reuse the corrupt file. The
    // rename is atomic on the same filesystem, so `out_path` only ever appears complete.
    let tmp_path = append_suffix(out_path, ".partial");
    // Run the fallible steps in a closure so we can delete the temp on ANY failure (short copy,
    // I/O error, or a killed rename) - like download_with_retry, so a partial is never left for a
    // later run to trust.
    let mut extract = || -> Result<u64, Box<dyn std::error::Error>> {
        let mut out_file = File::create(&tmp_path)?;
        let copied = io::copy(&mut entry, &mut out_file)?;
        // Flush and close before the rename: a crash then can't leave a renamed-but-empty file,
        // and a deferred write error (e.g. ENOSPC under delayed allocation) surfaces here.
        out_file.sync_all()?;
        drop(out_file);
        // A `Read` that hits EOF early returns `Ok`, so `io::copy` won't flag a short copy itself;
        // compare against the size the ZIP entry declares.
        if copied != expected {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!(
                "extracted {copied} bytes from '{entry_name}' in {} but the ZIP entry declares {expected}",
                zip_path.display(),
            )).into());
        }
        std::fs::rename(&tmp_path, out_path)?;
        Ok(copied)
    };
    let copied = match extract() {
        Ok(copied) => copied,
        Err(e) => {
            std::fs::remove_file(&tmp_path).ok();
            return Err(e);
        }
    };

    let size_mb = copied as f64 / (1024.0 * 1024.0);
    eprintln!("Extracted '{entry_name}' -> {} ({size_mb:.1} MB)", out_path.display());
    Ok(())
}

/// Find the first entry matching `glob_pattern` (or the first non-directory
/// entry when `None`). Returns `(entry_name, index)`. We look up by index
/// twice (here, then again in `extract_from_zip_to`) because `ZipArchive::by_index`
/// holds a mutable borrow and we need the name before deciding the output path.
fn find_zip_entry(
    zip_path: &Path,
    glob_pattern: Option<&str>,
) -> Result<(String, usize), Box<dyn std::error::Error>> {
    let file = File::open(zip_path)?;
    let reader = BufReader::new(file);
    let mut archive = zip::ZipArchive::new(reader)?;

    let matching_index = (0..archive.len())
        .find(|&i| {
            let Ok(entry) = archive.by_index(i) else { return false };
            let name = entry.name();
            if let Some(pattern) = glob_pattern {
                glob_match(pattern, name)
            } else {
                !name.ends_with('/')
            }
        })
        .ok_or_else(|| {
            let msg = if let Some(p) = glob_pattern {
                format!("No file matching '{p}' found in ZIP")
            } else {
                "ZIP archive is empty".to_string()
            };
            io::Error::new(io::ErrorKind::NotFound, msg)
        })?;

    let name = archive.by_index(matching_index)?.name().to_string();
    Ok((name, matching_index))
}

/// Simple glob matching supporting only `*` wildcards.
fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn test_glob_match_wildcard_all() {
        assert!(glob_match("*", "anything.txt"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn test_glob_match_suffix() {
        assert!(glob_match("*.csv", "data.csv"));
        assert!(glob_match("*.csv", "path/to/data.csv"));
        assert!(!glob_match("*.csv", "data.xml"));
        assert!(!glob_match("*.csv", "csv"));
    }

    #[test]
    fn test_glob_match_prefix() {
        assert!(glob_match("data*", "data.csv"));
        assert!(glob_match("data*", "data_file.xml"));
        assert!(!glob_match("data*", "other.csv"));
    }

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("data.csv", "data.csv"));
        assert!(!glob_match("data.csv", "other.csv"));
    }

    #[test]
    fn test_make_temp_path_with_extension() {
        let path = make_temp_path("csv");
        assert!(path.to_string_lossy().ends_with(".csv"));
        assert!(path.to_string_lossy().contains("nominatim-converter-"));
    }

    #[test]
    fn test_make_temp_path_empty_extension() {
        let path = make_temp_path("");
        assert!(path.to_string_lossy().ends_with(".tmp"));
    }

    #[test]
    fn test_make_temp_path_unique() {
        let p1 = make_temp_path("txt");
        let p2 = make_temp_path("txt");
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_resolve_input_local_file() {
        let path = Path::new("/some/local/file.csv");
        let resolved = resolve_input(path, Some("*.csv"), &CacheOptions::default()).unwrap();
        assert_eq!(resolved.path(), path);
    }

    #[test]
    fn test_resolve_input_relative_path() {
        let path = Path::new("relative/file.xml");
        let resolved = resolve_input(path, None, &CacheOptions::default()).unwrap();
        assert_eq!(resolved.path(), path);
    }

    #[test]
    fn test_resolve_input_local_path_bypasses_cache() {
        // A local path should be returned as-is, regardless of cache settings.
        let dir = std::env::temp_dir();
        let cache = CacheOptions::new(Some(&dir), true);
        let local = Path::new("/some/local/file.csv");
        let resolved = resolve_input(local, Some("*.csv"), &cache).unwrap();
        assert_eq!(resolved.path(), local);
    }

    #[test]
    fn test_resolve_input_local_zip_is_extracted() {
        // A local .zip must be extracted (matching the URL path and the CLI help), not
        // handed to the converter as raw bytes.
        let zip = create_test_zip(&[("data.gml", b"<gml/>")]);
        let resolved = resolve_input(&zip, Some("*.gml"), &CacheOptions::default()).unwrap();
        assert_ne!(resolved.path(), zip.as_path(), "zip should be extracted, not returned as-is");
        assert_eq!(std::fs::read(resolved.path()).unwrap(), b"<gml/>");

        // The extracted temp is cleaned up when the ResolvedInput drops.
        let extracted = resolved.path().to_path_buf();
        drop(resolved);
        assert!(!extracted.exists(), "extracted temp should be removed on drop");
        std::fs::remove_file(&zip).ok();
    }

    #[test]
    fn test_is_cached_returns_false_when_no_cache_dir() {
        let cache = CacheOptions::default();
        assert!(!is_cached("https://example.com/foo.zip", &cache));
    }

    #[test]
    fn test_is_cached_returns_false_when_file_missing() {
        let dir = std::env::temp_dir().join("nc-is-cached-miss");
        std::fs::create_dir_all(&dir).unwrap();
        let cache = CacheOptions::new(Some(&dir), false);
        assert!(!is_cached("https://example.com/unlikely-to-exist.zip", &cache));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_is_cached_detects_raw_and_extracted_entries() {
        let dir = std::env::temp_dir().join(format!("nc-is-cached-hit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = CacheOptions::new(Some(&dir), false);
        let url = "https://example.com/foo.zip";

        // Neither exists -> not cached.
        assert!(!is_cached(url, &cache));

        // Create the raw entry at the expected path.
        let parsed = parse_url(url);
        let raw = cache_path_in(&dir, &parsed.normalized, &parsed.basename);
        File::create(&raw).unwrap();
        assert!(is_cached(url, &cache));
        std::fs::remove_file(&raw).unwrap();

        // Only the extracted sibling exists -> still cached.
        let extracted = append_suffix(&raw, ".extracted");
        File::create(&extracted).unwrap();
        assert!(is_cached(url, &cache));
        std::fs::remove_file(&extracted).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_download_to_file() {
        let data = b"hello world test data";
        let reader = io::Cursor::new(data);
        let path = make_temp_path("txt");

        download_to_file(reader, &path, Some(data.len() as u64)).unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, data);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_download_to_file_no_content_length() {
        let data = b"some content";
        let reader = io::Cursor::new(data);
        let path = make_temp_path("txt");

        download_to_file(reader, &path, None).unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, data);
        std::fs::remove_file(&path).unwrap();
    }

    fn create_test_zip(files: &[(&str, &[u8])]) -> PathBuf {
        let path = make_temp_path("zip");
        let file = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn test_extract_from_zip_with_glob() {
        let zip_path = create_test_zip(&[
            ("readme.txt", b"ignore me"),
            ("data.csv", b"col1,col2\na,b"),
        ]);

        let extracted = extract_from_zip(&zip_path, Some("*.csv")).unwrap();
        let contents = std::fs::read_to_string(&extracted).unwrap();
        assert_eq!(contents, "col1,col2\na,b");
        assert!(extracted.to_string_lossy().ends_with(".csv"));

        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(&extracted).unwrap();
    }

    #[test]
    fn test_extract_from_zip_no_glob_picks_first_file() {
        let zip_path = create_test_zip(&[
            ("first.xml", b"<root/>"),
            ("second.txt", b"text"),
        ]);

        let extracted = extract_from_zip(&zip_path, None).unwrap();
        let contents = std::fs::read_to_string(&extracted).unwrap();
        assert_eq!(contents, "<root/>");

        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(&extracted).unwrap();
    }

    #[test]
    fn test_extract_from_zip_no_match() {
        let zip_path = create_test_zip(&[
            ("data.xml", b"<root/>"),
        ]);

        let result = extract_from_zip(&zip_path, Some("*.csv"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No file matching"));

        std::fs::remove_file(&zip_path).unwrap();
    }

    #[test]
    fn test_extract_from_zip_skips_directories() {
        let zip_path = create_test_zip(&[
            ("subdir/data.gml", b"<gml/>"),
        ]);

        let extracted = extract_from_zip(&zip_path, Some("*.gml")).unwrap();
        let contents = std::fs::read_to_string(&extracted).unwrap();
        assert_eq!(contents, "<gml/>");

        std::fs::remove_file(&zip_path).unwrap();
        std::fs::remove_file(&extracted).unwrap();
    }

    #[test]
    fn test_extract_to_cleans_up_partial_on_success() {
        let zip_path = create_test_zip(&[("data.gml", b"<gml>hello</gml>")]);
        let out = make_temp_path("gml");
        extract_from_zip_to(&zip_path, Some("*.gml"), &out).unwrap();

        assert_eq!(std::fs::read(&out).unwrap(), b"<gml>hello</gml>");
        assert!(!append_suffix(&out, ".partial").exists(), "no .partial temp should remain");

        std::fs::remove_file(&zip_path).ok();
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn test_resolved_input_temp_cleans_up_on_drop() {
        let path = make_temp_path("txt");
        File::create(&path).unwrap();
        assert!(path.exists());
        {
            let _resolved = ResolvedInput::temp(path.clone());
        }
        assert!(!path.exists());
    }

    #[test]
    fn test_resolved_input_persistent_survives_drop() {
        let path = make_temp_path("txt");
        File::create(&path).unwrap();
        assert!(path.exists());
        {
            let _resolved = ResolvedInput::persistent(path.clone());
        }
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_cache_options_default_is_disabled() {
        let opts = CacheOptions::default();
        assert!(opts.dir().is_none());
    }

    #[test]
    fn test_cache_options_empty_path_treated_as_unset() {
        let empty = Path::new("");
        let opts = CacheOptions::new(Some(empty), false);
        assert!(opts.dir().is_none(), "empty path should be treated as no cache");
    }

    #[test]
    fn test_cache_options_real_path_kept() {
        let dir = Path::new("/tmp/nc");
        let opts = CacheOptions::new(Some(dir), false);
        assert_eq!(opts.dir(), Some(dir));
    }

    #[test]
    fn test_cache_path_distinguishes_same_basename_different_urls() {
        let dir = Path::new("/tmp/nc");
        let a_parsed = parse_url("https://a.example.com/data.zip");
        let b_parsed = parse_url("https://b.example.com/data.zip");
        let a = cache_path_in(dir, &a_parsed.normalized, &a_parsed.basename);
        let b = cache_path_in(dir, &b_parsed.normalized, &b_parsed.basename);
        assert_ne!(a, b);
    }

    #[test]
    fn test_cache_path_stable_for_same_url() {
        let dir = Path::new("/tmp/nc");
        let p1 = parse_url("https://example.com/data.zip");
        let p2 = parse_url("https://example.com/data.zip");
        let a = cache_path_in(dir, &p1.normalized, &p1.basename);
        let b = cache_path_in(dir, &p2.normalized, &p2.basename);
        assert_eq!(a, b);
    }

    #[test]
    fn test_cache_path_preserves_basename() {
        let dir = Path::new("/tmp/nc");
        let p = parse_url("https://example.com/path/norway.osm.pbf");
        let out = cache_path_in(dir, &p.normalized, &p.basename);
        assert_eq!(out.parent().unwrap(), dir);
        let name = out.file_name().unwrap().to_string_lossy();
        assert!(name.ends_with("-norway.osm.pbf"), "got {name}");
    }

    #[test]
    fn test_parse_url_strips_fragment() {
        let p = parse_url("https://example.com/foo.zip#section");
        assert_eq!(p.normalized, "https://example.com/foo.zip");
        assert_eq!(p.basename, "foo.zip");
    }

    #[test]
    fn test_parse_url_lowercases_scheme_and_authority() {
        let p = parse_url("HTTPS://Example.COM/Path/File.ZIP");
        assert_eq!(p.normalized, "https://example.com/Path/File.ZIP");
        assert_eq!(p.basename, "File.ZIP");
        assert!(p.is_zip, "is_zip should be case-insensitive");
    }

    #[test]
    fn test_parse_url_basename_ignores_query_string() {
        let p = parse_url("https://example.com/foo.pbf?token=abc");
        assert_eq!(p.basename, "foo.pbf");
        assert_eq!(p.extension, "pbf");
    }

    #[test]
    fn test_parse_url_query_is_part_of_normalized_cache_key() {
        // Different queries should cache separately -- they often select content.
        let p1 = parse_url("https://example.com/foo.zip?v=1");
        let p2 = parse_url("https://example.com/foo.zip?v=2");
        assert_ne!(p1.normalized, p2.normalized);
    }

    #[test]
    fn test_parse_url_normalizes_case_insensitively_for_caching() {
        // Same URL with different scheme/host casing should produce the same cache key.
        let p1 = parse_url("HTTPS://EXAMPLE.com/data.zip");
        let p2 = parse_url("https://example.COM/data.zip");
        assert_eq!(p1.normalized, p2.normalized);
    }

    #[test]
    fn test_parse_url_no_basename_falls_back_to_download() {
        let p = parse_url("https://example.com/");
        assert_eq!(p.basename, "download");
        assert!(!p.is_zip);
    }

    #[test]
    fn test_append_suffix() {
        assert_eq!(
            append_suffix(Path::new("/tmp/foo.zip"), ".extracted"),
            PathBuf::from("/tmp/foo.zip.extracted")
        );
    }

    #[test]
    fn test_is_retryable_http_status_codes() {
        let retryable = [500u16, 502, 503, 408, 429];
        for code in retryable {
            let err: Box<dyn std::error::Error> = Box::new(ureq::Error::StatusCode(code));
            assert!(is_retryable(err.as_ref()), "HTTP {code} should be retryable");
        }
        let permanent = [400u16, 401, 403, 404];
        for code in permanent {
            let err: Box<dyn std::error::Error> = Box::new(ureq::Error::StatusCode(code));
            assert!(!is_retryable(err.as_ref()), "HTTP {code} should not be retryable");
        }
    }

    #[test]
    fn test_is_retryable_io_and_string_errors() {
        let io_err: Box<dyn std::error::Error> =
            Box::new(io::Error::new(io::ErrorKind::ConnectionReset, "reset"));
        assert!(is_retryable(io_err.as_ref()));

        let string_err: Box<dyn std::error::Error> = "something else".into();
        assert!(is_retryable(string_err.as_ref()));
    }

    #[test]
    fn test_is_retryable_walks_source_chain() {
        // A ureq status error wrapped in an io::Error should still be
        // classified by the inner status code.
        let inner = ureq::Error::StatusCode(404);
        let wrapped: Box<dyn std::error::Error> = Box::new(io::Error::other(inner));
        assert!(!is_retryable(wrapped.as_ref()));
    }

    #[test]
    fn test_download_with_retry_gives_up_on_permanent_error() {
        let calls = Cell::new(0u32);
        let path = make_temp_path("txt");
        let result = download_with_retry("https://example.com/x", &path, |_url| {
            calls.set(calls.get() + 1);
            Err(Box::new(ureq::Error::StatusCode(404)) as Box<dyn std::error::Error>)
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 1, "permanent errors must not be retried");
        assert!(!path.exists());
    }

    #[test]
    fn test_download_with_retry_recovers_from_transient_error() {
        let calls = Cell::new(0u32);
        let path = make_temp_path("txt");
        let result = download_with_retry("https://example.com/x", &path, |_url| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Err(Box::new(io::Error::new(io::ErrorKind::ConnectionReset, "reset"))
                    as Box<dyn std::error::Error>)
            } else {
                let data = b"payload".to_vec();
                Ok(DownloadStream::new(Box::new(io::Cursor::new(data)), Some(7)))
            }
        });
        assert!(result.is_ok());
        assert_eq!(calls.get(), 2);
        assert_eq!(std::fs::read(&path).unwrap(), b"payload");
        std::fs::remove_file(&path).unwrap();
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::ConnectionReset, "mid-stream failure"))
        }
    }

    #[test]
    fn test_download_with_retry_removes_partial_file_after_final_failure() {
        let calls = Cell::new(0u32);
        let path = make_temp_path("txt");
        let result = download_with_retry("https://example.com/x", &path, |_url| {
            calls.set(calls.get() + 1);
            // Stream that yields some bytes, then fails mid-download.
            let good: Box<dyn Read> = Box::new(io::Cursor::new(b"partial".to_vec()));
            let bad: Box<dyn Read> = Box::new(FailingReader);
            Ok(DownloadStream::new(Box::new(good.chain(bad)), None))
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), DOWNLOAD_ATTEMPTS, "mid-stream failures should be retried");
        assert!(!path.exists(), "partial download must not be left behind");
    }

}
