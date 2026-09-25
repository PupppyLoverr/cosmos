//! "Sign in with ChatGPT" — use a ChatGPT Plus/Pro plan instead of an API key.
//!
//! Ported from the Codex CLI (`codex-rs/login`, Apache-2.0) and pi
//! (`packages/ai/src/auth/oauth/openai-codex.ts`, MIT): an OAuth 2.0
//! authorization-code flow with PKCE against auth.openai.com using the Codex
//! CLI's public client id, a one-shot callback server on localhost:1455, and
//! refresh-token renewal. Requests then go to the ChatGPT backend's Responses
//! endpoint with the account id taken from the access token's claims.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
const PORT: u16 = 1455;
const SCOPE: &str = "openid profile email offline_access";
const CLAIMS: &str = "https://api.openai.com/auth";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    /// unix seconds
    pub expires_at: u64,
    pub account_id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS randomness");
    b
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes::<64>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn redirect_uri() -> String {
    format!("http://localhost:{PORT}/auth/callback")
}

pub fn authorize_url(p: &Pkce, state: &str) -> String {
    let q = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", &redirect_uri()),
        ("scope", SCOPE),
        ("code_challenge", &p.challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "zcli"),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", enc(v)))
    .collect::<Vec<_>>()
    .join("&");
    format!("{ISSUER}/oauth/authorize?{q}")
}

/// Decode a JWT's payload (no signature check — the token came from the
/// issuer over TLS and is only read for its account claims).
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).or_else(|_| URL_SAFE.decode(payload)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn tokens_from(v: &Value, prev_refresh: Option<&str>) -> Result<Tokens> {
    let access = v["access_token"].as_str().context("token response missing access_token")?.to_string();
    let refresh = v["refresh_token"].as_str().map(str::to_string).or(prev_refresh.map(str::to_string)).context("token response missing refresh_token")?;
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
    let claims = jwt_claims(&access).context("access token is not a JWT")?;
    let auth = &claims[CLAIMS];
    let account_id = auth["chatgpt_account_id"].as_str().context("no ChatGPT account on this login")?.to_string();
    let id_claims = v["id_token"].as_str().and_then(jwt_claims);
    let email = id_claims.as_ref().and_then(|c| c["email"].as_str()).or(claims["email"].as_str()).map(str::to_string);
    let plan = auth["chatgpt_plan_type"]
        .as_str()
        .or_else(|| id_claims.as_ref().and_then(|c| c[CLAIMS]["chatgpt_plan_type"].as_str()))
        .map(str::to_string);
    Ok(Tokens { access, refresh, expires_at: now() + expires_in, account_id, email, plan })
}

fn token_request(form: &[(&str, &str)]) -> Result<Value> {
    let body = form.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect::<Vec<_>>().join("&");
    let mut resp = ureq::post(&format!("{ISSUER}/oauth/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .config()
        .http_status_as_error(false)
        .build()
        .send(body.as_bytes())
        .context("could not reach auth.openai.com")?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status >= 400 {
        bail!("ChatGPT sign-in failed ({status}): {}", text.chars().take(300).collect::<String>());
    }
    Ok(serde_json::from_str(&text)?)
}

pub fn exchange_code(code: &str, verifier: &str) -> Result<Tokens> {
    let redirect = redirect_uri();
    let v = token_request(&[
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", &redirect),
    ])?;
    tokens_from(&v, None)
}

pub fn refresh(t: &Tokens) -> Result<Tokens> {
    let v = token_request(&[("grant_type", "refresh_token"), ("refresh_token", &t.refresh), ("client_id", CLIENT_ID)])?;
    let mut n = tokens_from(&v, Some(&t.refresh))?;
    if n.email.is_none() {
        n.email = t.email.clone();
    }
    if n.plan.is_none() {
        n.plan = t.plan.clone();
    }
    Ok(n)
}

pub fn load() -> Option<Tokens> {
    crate::creds::load_file().chatgpt
}

pub fn save(t: &Tokens) -> Result<()> {
    let mut f = crate::creds::load_file();
    f.chatgpt = Some(t.clone());
    crate::creds::save_file(&f)
}

pub fn logout() -> Result<()> {
    let mut f = crate::creds::load_file();
    f.chatgpt = None;
    crate::creds::save_file(&f)
}

/// The signed-in session, refreshed when it expires within five minutes.
pub fn fresh() -> Option<Tokens> {
    let t = load()?;
    if t.expires_at > now() + 300 {
        return Some(t);
    }
    match refresh(&t) {
        Ok(n) => {
            let _ = save(&n);
            Some(n)
        }
        Err(_) => Some(t), // let the request fail with a clear 401
    }
}

/// Open `url` in the default browser.
pub fn open_browser(url: &str) -> bool {
    #[cfg(windows)]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.is_ok()
}

const PAGE: &str = "<!doctype html><meta charset=utf-8><title>zcli</title><body style=\"font:16px system-ui;background:#0b0b0c;color:#eee;display:grid;place-items:center;height:100vh;margin:0\"><div><h2 style=font-weight:600>Signed in to zcli</h2><p style=color:#8a8a8a>You can close this tab and return to the terminal.</p></div>";

fn query_param(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| {
            let mut out = Vec::new();
            let b = v.as_bytes();
            let mut i = 0;
            while i < b.len() {
                match b[i] {
                    b'%' if i + 2 < b.len() => {
                        let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                        out.push(h.unwrap_or(b'%'));
                        i += 3;
                        continue;
                    }
                    b'+' => out.push(b' '),
                    c => out.push(c),
                }
                i += 1;
            }
            String::from_utf8_lossy(&out).to_string()
        })
    })
}

/// Full browser sign-in. `on_url` receives the authorize URL (the caller
/// shows it and opens the browser). Blocks until the callback arrives, the
/// user cancels, or five minutes pass. Saves and returns the session.
pub fn login(cancel: &AtomicBool, on_url: impl FnOnce(&str)) -> Result<Tokens> {
    let listener = TcpListener::bind(("127.0.0.1", PORT)).with_context(|| format!("port {PORT} is busy — close other Codex/zcli sign-ins and retry"))?;
    listener.set_nonblocking(true)?;
    let p = pkce();
    let state = URL_SAFE_NO_PAD.encode(random_bytes::<16>());
    on_url(&authorize_url(&p, &state));
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("sign-in cancelled");
        }
        if Instant::now() > deadline {
            bail!("sign-in timed out");
        }
        let (stream, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_nonblocking(false)?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let path = line.split_whitespace().nth(1).unwrap_or("");
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let mut s = &stream;
        if route != "/auth/callback" {
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let reply = |s: &mut &std::net::TcpStream, body: &str| {
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        };
        if query_param(query, "state").as_deref() != Some(state.as_str()) {
            reply(&mut s, "State mismatch — start the sign-in again from zcli.");
            bail!("sign-in state mismatch");
        }
        if let Some(err) = query_param(query, "error") {
            reply(&mut s, &format!("Sign-in failed: {err}"));
            bail!("sign-in failed: {err}");
        }
        let code = query_param(query, "code").context("callback had no code")?;
        let tokens = match exchange_code(&code, &p.verifier) {
            Ok(t) => {
                reply(&mut s, PAGE);
                t
            }
            Err(e) => {
                reply(&mut s, &format!("Sign-in failed: {e}"));
                return Err(e);
            }
        };
        save(&tokens)?;
        return Ok(tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jwt(payload: &Value) -> String {
        format!("x.{}.y", URL_SAFE_NO_PAD.encode(payload.to_string()))
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = pkce();
        assert!(p.verifier.len() >= 43);
        assert_eq!(p.challenge, URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes())));
    }

    #[test]
    fn authorize_url_carries_codex_parameters() {
        let p = pkce();
        let u = authorize_url(&p, "st8");
        assert!(u.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(u.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(u.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(u.contains("code_challenge_method=S256") && u.contains("state=st8"));
        assert!(u.contains("codex_cli_simplified_flow=true"));
    }

    #[test]
    fn tokens_read_account_and_plan_claims() {
        let access = fake_jwt(&serde_json::json!({CLAIMS: {"chatgpt_account_id": "acc_1", "chatgpt_plan_type": "plus"}}));
        let id = fake_jwt(&serde_json::json!({"email": "a@b.c"}));
        let t = tokens_from(&serde_json::json!({"access_token": access, "refresh_token": "r1", "id_token": id, "expires_in": 60}), None).unwrap();
        assert_eq!(t.account_id, "acc_1");
        assert_eq!(t.plan.as_deref(), Some("plus"));
        assert_eq!(t.email.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn callback_query_decoding() {
        assert_eq!(query_param("code=a%2Fb+c&state=s", "code").as_deref(), Some("a/b c"));
        assert_eq!(query_param("code=x", "state"), None);
    }
}
