//! Anonymous HTTP exchanges: `yt.fetch` (BotGuard), `addon.fetch` and
//! `lyrics.fetch`. Port of `web/extension/lib/rules.ts` + `lyricsFetch.ts`
//! plus the serialiser from `ytFetch.ts` — in Rust the "session rules" are
//! just headers set on the request, and CORS never applies.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use regex::Regex;

use crate::store::MUSIC_ORIGIN;
use crate::types::{HttpRequest, HttpResponse};

/// The Chrome identity the page's own fetches present, matching the agent
/// window and the signed clients: to YouTube the desktop is a Chrome browser.
pub const CHROME_UA: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) ",
    "Chrome/138.0.0.0 Safari/537.36"
);

/// Headers some lyric hosts need that a browser fetch may not set
/// (lyricsFetch.ts RULES): Apple's catalogue search and Musixmatch's web
/// client with the signing key its browserGet carries.
fn lyric_headers(origin: &str) -> Vec<(&'static str, String)> {
    if origin == "https://amp-api.music.apple.com" {
        return vec![
            ("Origin", "https://music.apple.com".into()),
            ("Referer", "https://music.apple.com/".into()),
        ];
    }
    if origin.ends_with("musixmatch.com") {
        return vec![("Cookie", "mxm_bab=AB".into())];
    }
    Vec::new()
}

fn to_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn from_base64(value: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|e| e.to_string())
}

/// Serialises a response the way `ytFetch.serialise` does: text for textual
/// content types, base64 for everything else.
pub async fn serialise(response: reqwest::Response, html_is_text: bool) -> HttpResponse {
    let status = response.status().as_u16();
    let url = response.url().to_string();
    let mut headers = HashMap::new();
    for (name, value) in response.headers().iter() {
        if let Ok(v) = value.to_str() {
            headers.insert(name.as_str().to_string(), v.to_string());
        }
    }
    let content_type = headers.get("content-type").cloned().unwrap_or_default().to_lowercase();
    let textual = Regex::new(r"json|text|javascript|xml").unwrap().is_match(&content_type)
        || (html_is_text && content_type.is_empty());
    if textual {
        let body = response.text().await.unwrap_or_default();
        HttpResponse { status, headers, url, body, body_encoding: "text" }
    } else {
        let body = to_base64(&response.bytes().await.unwrap_or_default());
        HttpResponse { status, headers, url, body, body_encoding: "base64" }
    }
}

/// A no-cookie client shared by every anonymous exchange.
pub fn anon_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) ",
            "Chrome/138.0.0.0 Safari/537.36 BitChord/"
        ))
        .redirect(reqwest::redirect::Policy::default())
        .timeout(Duration::from_secs(30))
        .build()
        .expect("anon client")
}

pub type SharedClient = Arc<reqwest::Client>;

/// Where an anonymous `yt.fetch` may go: BotGuard, which a page can't call with YouTube's Origin.
const YT_ANON_ALLOWED: &[&str] = &["https://www.youtube.com/api/jnn/"];

/// Where a signed one may go: InnerTube only, never an account or settings page.
pub const YT_SIGNED_ALLOWED: &[&str] = &["https://music.youtube.com/youtubei/v1/", "https://www.youtube.com/youtubei/v1/"];

/// The play-reporting pings a player response nominates (playbackTracking):
/// playback, watchtime and atr, GET only. They're what puts a play in the
/// account's YouTube Music history (PlaybackTracker.kt on the phone).
pub fn is_stats_url(url: &str) -> bool {
    Regex::new(r"^https://(s|music|www)\.youtube\.com/api/stats/(playback|watchtime|atr)(\?|$)")
        .unwrap()
        .is_match(url)
}

fn forbidden(url: &str, what: &str) -> (String, String) {
    let origin = url::Url::parse(url).map(|u| format!("{}{}", u.origin().ascii_serialization(), u.path())).unwrap_or_default();
    (format!("{what} doesn't serve {origin}"), "forbidden".into())
}

/// `yt.fetch` without `auth` (ytFetch.ytFetch): BotGuard only, headers pass through.
pub async fn yt_fetch_anon(client: &reqwest::Client, request: &HttpRequest) -> Result<HttpResponse, (String, String)> {
    if !YT_ANON_ALLOWED.iter().any(|base| request.url.starts_with(base)) {
        return Err(forbidden(&request.url, "yt.fetch"));
    }
    send_exchange(client, request, Vec::new(), false).await
}

/// `addon.fetch`: a module source the page can't reach itself. No host list here
/// (the Android app's sources are open-ended), so every https origin is served.
pub async fn addon_fetch(client: &reqwest::Client, request: &HttpRequest) -> Result<HttpResponse, (String, String)> {
    send_exchange(client, request, Vec::new(), false).await
}

/// `lyrics.fetch` (lyricsFetch.lyricsFetch): only the known lyric origins,
/// with their extra headers, and a 15 s deadline.
pub async fn lyrics_fetch(client: &reqwest::Client, request: &HttpRequest, granted: &[String]) -> Result<HttpResponse, (String, String)> {
    let parsed = url::Url::parse(&request.url).map_err(|_| ("bad url".into(), "internal".into()))?;
    let origin = parsed.origin().ascii_serialization();
    if !granted.iter().any(|o| o == &origin) {
        return Err((
            format!("BitChord needs permission to reach {} for lyrics.", parsed.host_str().unwrap_or_default()),
            "permission".into(),
        ));
    }
    let extra = lyric_headers(&origin)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect::<Vec<_>>();
    send_exchange(client, request, extra, true).await
}

async fn send_exchange(
    client: &reqwest::Client,
    request: &HttpRequest,
    mut extra: Vec<(String, String)>,
    short_deadline: bool,
) -> Result<HttpResponse, (String, String)> {
    let method = request.method.clone().unwrap_or_else(|| "GET".into());
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| (e.to_string(), "internal".into()))?;
    let mut builder = client.request(method, &request.url);
    if let Some(headers) = &request.headers {
        for (key, value) in headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
    }
    for (key, value) in extra.drain(..) {
        builder = builder.header(key.as_str(), value.as_str());
    }
    if let Some(body) = &request.body {
        if request.body_encoding.as_deref() == Some("base64") {
            let bytes = from_base64(body).map_err(|e| (e, "internal".into()))?;
            builder = builder.body(bytes);
        } else {
            builder = builder.body(body.clone());
        }
    }
    if short_deadline {
        builder = builder.timeout(Duration::from_secs(15));
    }
    let response = builder.send().await.map_err(|e| (network_message(&e), "network".into()))?;
    Ok(serialise(response, true).await)
}

fn network_message(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "The request timed out".into()
    } else if e.is_connect() {
        format!("Could not connect: {e}")
    } else {
        e.to_string()
    }
}

// ---- The page's own fetches (the extension's DNR rules) -------------------------
//
// The web app fetches YouTube from the page itself, and in the browser those
// fetches were made to work by the extension's declarativeNetRequest rules
// (`web/extension/lib/rules.ts::youtubeRules`): the request left with the
// YouTube headers a page cannot set itself, and the response came back with
// Access-Control-Allow-Origin. youtubei answers CORS with *nothing at all*
// (web/fixtures/index.json: `acao` is none on every youtubei capture), and
// googlevideo serves the YouTube origin alone — so without the rules every
// browse, search and media request dies with "Failed to fetch". In the
// desktop the page's matched fetches come here instead: same headers, no
// CORS to satisfy.

/// The request headers the DNR rules put on a page fetch to [url], one for
/// one from `youtubeRules`:
///
/// - `music.youtube.com/youtubei/` and `www.youtube.com/youtubei/` are
///   impersonated as their own web clients (Origin, Referer, X-Origin);
/// - `www.youtube.com/sw.js_data`, `/iframe_api` and `/s/player/` pass with
///   no header changes — YouTube.js reads them to build a session and to
///   decipher stream URLs;
/// - every `googlevideo.com` host is impersonated as YouTube's web client
///   (Origin and Referer only — googlevideo refuses other origins), except
///   direct URLs minted for VISIONOS, which take no Origin or Referer at
///   all: a YouTube web Origin on them is refused with 403 mid-track, as
///   InnerTubeX knows (the rule had priority 2 over the catch-all);
/// - `www.youtube.com/api/jnn/` (BotGuard's Create) is deliberately *not*
///   matched, like in the rules: it allows any origin and refuses a
///   rewritten YouTube one, so the page fetches it itself.
///
/// An `Ok` of an empty vec is a matched rule with no header changes; an
/// `Err` is a URL no rule matched, which the shell must not intercept.
pub fn page_request_headers(url: &str) -> Result<Vec<(&'static str, String)>, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    if parsed.scheme() != "https" {
        return Err(format!("page.fetch serves https only, not {url}"));
    }
    let host = parsed.host_str().unwrap_or_default();
    let path = parsed.path();
    if (host == "music.youtube.com" || host == "www.youtube.com") && path.starts_with("/youtubei/") {
        let origin = if host == "music.youtube.com" { MUSIC_ORIGIN } else { "https://www.youtube.com" };
        return Ok(impersonate(origin, true));
    }
    if host == "www.youtube.com"
        && (path == "/sw.js_data" || path == "/iframe_api" || path.starts_with("/s/player/"))
    {
        return Ok(Vec::new());
    }
    if host == "googlevideo.com" || host.ends_with(".googlevideo.com") {
        // The DNR rule was the wildcard `||googlevideo.com/*c=VISIONOS`:
        // the client tag rides in the URL, not in a fixed position.
        if url.contains("c=VISIONOS") {
            return Ok(Vec::new());
        }
        return Ok(impersonate("https://www.youtube.com", false));
    }
    Err(format!("page.fetch doesn't serve {url}"))
}

/// The DNR `impersonate` helper: the YouTube web client's Origin and Referer,
/// plus X-Origin only where the rule added it (InnerTube, not googlevideo).
fn impersonate(origin: &str, with_x_origin: bool) -> Vec<(&'static str, String)> {
    let mut headers = vec![("Origin", origin.to_string()), ("Referer", format!("{origin}/"))];
    if with_x_origin {
        headers.push(("X-Origin", origin.to_string()));
    }
    headers
}

/// A no-cookie client for the page's own fetches: the page sends its YouTube
/// requests anonymous (`credentials: 'omit'`), and this is what carries them.
pub fn page_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .expect("page client")
}

/// The page's own fetch, as the browser would have sent it under the rules.
/// With `fallback`, anything https is served with the page's headers and no
/// impersonation — the desktop's answer for cross-origin hosts that answer
/// no CORS at all (the Listen Together server; the site reached it
/// same-origin). The page only asks for the fallback after its own fetch
/// died as a network error, so the browser's behaviour stays the norm.
pub async fn page_fetch(
    client: &reqwest::Client,
    request: &HttpRequest,
    fallback: bool,
) -> Result<HttpResponse, (String, String)> {
    let extra = if fallback {
        Vec::new()
    } else {
        page_request_headers(&request.url).map_err(|message| (message, "forbidden".into()))?
    };
    let media = url::Url::parse(&request.url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h == "googlevideo.com" || h.ends_with(".googlevideo.com")))
        .unwrap_or(false);
    let method = request.method.clone().unwrap_or_else(|| "GET".into());
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| (e.to_string(), "internal".into()))?;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::USER_AGENT, reqwest::header::HeaderValue::from_static(CHROME_UA));
    if let Some(map) = &request.headers {
        for (key, value) in map {
            let lower = key.to_ascii_lowercase();
            // What the network layer owns, the page never really set: a
            // browser strips them out of a fetch the same way.
            if matches!(lower.as_str(), "host" | "content-length" | "connection" | "user-agent" | "accept-encoding") {
                continue;
            }
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(lower.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                headers.insert(name, value);
            }
        }
    }
    for (key, value) in &extra {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(key.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        }
    }

    let mut builder = client.request(method, &request.url).headers(headers);
    // Media has no deadline of its own: Shaka aborts and retries on its own
    // schedule, and a hard limit here would cut slow first segments short.
    // Everything else gets the browser's own order of magnitude.
    builder = builder.timeout(Duration::from_secs(if media { 120 } else { 60 }));
    if let Some(body) = &request.body {
        if request.body_encoding.as_deref() == Some("base64") {
            let bytes = from_base64(body).map_err(|e| (e, "internal".into()))?;
            builder = builder.body(bytes);
        } else {
            builder = builder.body(body.clone());
        }
    }
    let response = builder.send().await.map_err(|e| (network_message(&e), "network".into()))?;
    Ok(serialise(response, false).await)
}
