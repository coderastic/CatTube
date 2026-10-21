//! Signing in the way the YouTube TV app does it: a code
//! shown on the TV, entered on a phone or computer (google.com/device), and
//! tokens that are refreshed as they run out. Several accounts can be kept.
//!
//! | What | Request |
//! |---|---|
//! | the TV app's client | `GET https://www.youtube.com/tv` → `base-js` script → `clientId:"…",…:"<secret>"` |
//! | a code | `POST https://www.youtube.com/o/oauth2/device/code` `{client_id, scope, device_id, device_model}` |
//! | signed in yet? | `POST https://www.youtube.com/o/oauth2/token` `{client_id, client_secret, code, grant_type: …device/1.0}` |
//! | fresh token | the same, `{refresh_token, grant_type: refresh_token}` |
//! | who | `POST /youtubei/v1/account/accounts_list` (signed in) |
//!
//! The client id is read from YouTube's own TV page each time it is needed,
//! not kept in this source. Tokens live in `accounts.conf` (mode 0600).

use crate::mpv::{Json, json_string, parse_json};
use std::path::{Path, PathBuf};

pub const TV_PAGE: &str = "https://www.youtube.com/tv";
pub const CODE_URL: &str = "https://www.youtube.com/o/oauth2/device/code";
pub const TOKEN_URL: &str = "https://www.youtube.com/o/oauth2/token";
const SCOPE: &str = "http://gdata.youtube.com https://www.googleapis.com/auth/youtube-paid-content";
const DEVICE_GRANT: &str = "http://oauth.net/grant_type/device/1.0";

/// The TV app's sign-in client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub secret: String,
}

/// The address of the TV app's script in the TV page: a plain
/// `<script id="base-js" src="…">`, or (for TV browsers) a script that
/// sets `s.src = '/s/_/kabuki/…/m=base'` and adds the tag itself.
pub fn base_js_path(tv_page: &str) -> Option<String> {
    let ok = |p: &str| {
        (p.starts_with('/') && !p.contains("..") && !p.contains('\\') && p.len() < 512)
            .then(|| p.to_string())
    };
    if let Some(at) = tv_page.find("id=\"base-js\"") {
        let rest = &tv_page[at..];
        if let Some(src) = rest.find("src=\"").map(|i| i + 5)
            && let Some(end) = rest[src..].find('"')
        {
            return ok(&rest[src..src + end]);
        }
    }
    tv_page
        .match_indices("'/s/_/kabuki/")
        .filter_map(|(i, _)| {
            let p = &tv_page[i + 1..];
            ok(&p[..p.find('\'')?])
        })
        .find(|p| p.ends_with("/m=base"))
}

/// `clientId:"…",<name>:"<secret>"` from the TV app's script.
pub fn parse_client(base_js: &str) -> Option<Client> {
    let at = base_js.find("clientId:\"")? + 10;
    let rest = &base_js[at..];
    let end = rest.find('"')?;
    let id = &rest[..end];
    // The secret is the next quoted value after the id.
    let after = &rest[end + 1..];
    let q1 = after.find(":\"")? + 2;
    if after[..q1].contains('}') || q1 > 40 {
        return None;
    }
    let q2 = after[q1..].find('"')?;
    let secret = &after[q1..q1 + q2];
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() < 200
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    };
    (ok(id) && ok(secret)).then(|| Client {
        id: id.to_string(),
        secret: secret.to_string(),
    })
}

/// A code for the TV: what to show, and what to ask with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// "WZY-WHH-FZQ".
    pub user_code: String,
    /// "https://www.google.com/device".
    pub url: String,
    pub interval: u64,
    pub expires_in: u64,
}

pub fn code_body(client: &Client, device_id: &str) -> String {
    format!(
        "{{\"client_id\":{},\"scope\":{},\"device_id\":{},\"device_model\":\"ytlr::\"}}",
        json_string(&client.id),
        json_string(SCOPE),
        json_string(device_id)
    )
}

pub fn parse_code(body: &str) -> Option<DeviceCode> {
    let v = parse_json(body.trim())?;
    let s = |k: &str| v.get(k).and_then(Json::as_str).map(str::to_string);
    let n = |k: &str| v.get(k).and_then(Json::as_f64).map(|f| f as u64);
    Some(DeviceCode {
        device_code: s("device_code")?,
        user_code: s("user_code")?,
        url: s("verification_url").unwrap_or_else(|| "https://www.google.com/device".into()),
        interval: n("interval").unwrap_or(5).clamp(1, 60),
        expires_in: n("expires_in").unwrap_or(1800),
    })
}

pub fn poll_body(client: &Client, device_code: &str) -> String {
    format!(
        "{{\"client_id\":{},\"client_secret\":{},\"code\":{},\"grant_type\":{}}}",
        json_string(&client.id),
        json_string(&client.secret),
        json_string(device_code),
        json_string(DEVICE_GRANT)
    )
}

pub fn refresh_body(client: &Client, refresh_token: &str) -> String {
    format!(
        "{{\"client_id\":{},\"client_secret\":{},\"refresh_token\":{},\"grant_type\":\"refresh_token\"}}",
        json_string(&client.id),
        json_string(&client.secret),
        json_string(refresh_token)
    )
}

/// What a token request said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReply {
    /// Signed in: the token, how long it lasts (s), and the refresh token
    /// (only on the first sign-in).
    Token {
        access: String,
        expires_in: u64,
        refresh: Option<String>,
    },
    /// Not entered yet: ask again.
    Pending,
    /// Asked too often: wait longer.
    SlowDown,
    /// Expired, refused, or a refresh token that no longer works.
    Failed(String),
}

pub fn parse_token(body: &str) -> TokenReply {
    let Some(v) = parse_json(body.trim()) else {
        return TokenReply::Failed("unreadable answer".into());
    };
    if let Some(err) = v.get("error").and_then(Json::as_str) {
        return match err {
            "authorization_pending" => TokenReply::Pending,
            "slow_down" => TokenReply::SlowDown,
            "expired_token" => TokenReply::Failed("The code expired".into()),
            "access_denied" => TokenReply::Failed("Sign-in was cancelled".into()),
            "invalid_grant" => TokenReply::Failed("Signed out (sign in again)".into()),
            other => TokenReply::Failed(other.to_string()),
        };
    }
    match v.get("access_token").and_then(Json::as_str) {
        Some(a) if !a.is_empty() && !a.contains(['\n', '\r']) => TokenReply::Token {
            access: a.to_string(),
            expires_in: v.get("expires_in").and_then(Json::as_f64).unwrap_or(3600.0) as u64,
            refresh: v
                .get("refresh_token")
                .and_then(Json::as_str)
                .filter(|r| !r.is_empty() && !r.contains(['\n', '\r']))
                .map(str::to_string),
        },
        _ => TokenReply::Failed("no token in the answer".into()),
    }
}

/// `account/accounts_list` → the signed-in name and picture.
pub fn parse_account_name(body: &str) -> Option<(String, String)> {
    let v = parse_json(body.trim())?;
    fn find<'a>(v: &'a Json, key: &str) -> Option<&'a Json> {
        match v {
            Json::Obj(kv) => kv
                .iter()
                .find_map(|(k, x)| if k == key { Some(x) } else { find(x, key) }),
            Json::Arr(a) => a.iter().find_map(|x| find(x, key)),
            _ => None,
        }
    }
    let item = find(&v, "accountItem").or_else(|| find(&v, "accountItemRenderer"))?;
    let name = crate::youtube::text(item.get("accountName"));
    let photo = item
        .get("accountPhoto")
        .and_then(|p| p.get("thumbnails"))
        .and_then(Json::as_arr)
        .and_then(|t| t.last())
        .and_then(|t| t.get("url"))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    (!name.is_empty()).then_some((name, photo))
}

pub fn accounts_list_body(ctx: &str) -> String {
    format!("{{\"context\":{ctx},\"accountReadMask\":{{\"returnOwner\":true}}}}")
}

// ---------------------------------------------------------------- accounts

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    pub name: String,
    pub photo: String,
    pub refresh_token: String,
    /// The current token and when it runs out (Unix seconds); not saved.
    pub access_token: String,
    pub expires_at: u64,
}

impl Account {
    /// The token is good for another minute at least.
    pub fn fresh(&self, now: u64) -> bool {
        !self.access_token.is_empty() && now + 60 < self.expires_at
    }
}

/// The signed-in accounts (`accounts.conf`, 0600) and the device id YouTube
/// knows this TV by.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Accounts {
    pub device_id: String,
    pub list: Vec<Account>,
    pub active: usize,
}

impl Accounts {
    pub fn active(&self) -> Option<&Account> {
        self.list.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut Account> {
        self.list.get_mut(self.active)
    }

    /// Add (or replace the same account) and make it the one in use.
    pub fn add(&mut self, a: Account) {
        if let Some(i) = self.list.iter().position(|x| {
            x.refresh_token == a.refresh_token || (!a.name.is_empty() && x.name == a.name)
        }) {
            self.list[i] = a;
            self.active = i;
        } else {
            self.list.push(a);
            self.active = self.list.len() - 1;
        }
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.list.len() {
            self.list.remove(i);
        }
        self.active = self.active.min(self.list.len().saturating_sub(1));
    }

    pub fn from_conf(text: &str) -> Self {
        let mut acc = Accounts::default();
        let mut cur: Option<Account> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[account]" {
                if let Some(a) = cur.take().filter(|a| !a.refresh_token.is_empty()) {
                    acc.list.push(a);
                }
                cur = Some(Account::default());
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim().to_string());
            match (&mut cur, k) {
                (None, "device_id") if v.len() <= 64 => acc.device_id = v,
                (None, "active") => acc.active = v.parse().unwrap_or(0),
                (Some(a), "name") => a.name = v,
                (Some(a), "photo") => a.photo = v,
                (Some(a), "refresh_token") => a.refresh_token = v,
                _ => {}
            }
        }
        if let Some(a) = cur.filter(|a| !a.refresh_token.is_empty()) {
            acc.list.push(a);
        }
        acc.active = acc.active.min(acc.list.len().saturating_sub(1));
        acc
    }

    pub fn to_conf(&self) -> String {
        let clean = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>();
        let mut s = format!(
            "# CatTube accounts (contains sign-in tokens)\ndevice_id = {}\nactive = {}\n",
            clean(&self.device_id),
            self.active
        );
        for a in &self.list {
            s.push_str(&format!(
                "\n[account]\nname = {}\nphoto = {}\nrefresh_token = {}\n",
                clean(&a.name),
                clean(&a.photo),
                clean(&a.refresh_token)
            ));
        }
        s
    }
}

pub fn accounts_path(config: &Path) -> PathBuf {
    config.join("accounts.conf")
}

pub fn load(config: &Path) -> Accounts {
    let mut a = std::fs::read_to_string(accounts_path(config))
        .map(|t| Accounts::from_conf(&t))
        .unwrap_or_default();
    if a.device_id.is_empty() {
        a.device_id = crate::util::random_hex(16).unwrap_or_else(|_| "cattube".into());
    }
    a
}

/// Save with mode 0600 (tokens), atomically.
pub fn save(config: &Path, a: &Accounts) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(config)?;
    let path = accounts_path(config);
    let tmp = path.with_extension("conf.tmp");
    let _ = std::fs::remove_file(&tmp);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(a.to_conf().as_bytes())?;
    }
    std::fs::rename(tmp, path)
}

/// A UUID-shaped device id from 16 random hex bytes.
pub fn device_uuid(hex32: &str) -> String {
    let h: String = hex32
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(32)
        .collect();
    if h.len() < 32 {
        return "00000000-0000-4000-8000-000000000000".into();
    }
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_from_the_tv_app() {
        let page = r#"<script id="base-js" src="/s/_/kabuki/_/js/k=kabuki.base/m=base" nonce="x"></script>"#;
        assert_eq!(
            base_js_path(page).as_deref(),
            Some("/s/_/kabuki/_/js/k=kabuki.base/m=base")
        );
        assert_eq!(base_js_path(r#"id="base-js" src="https://evil/x""#), None);
        // What a TV browser gets: the tag is made by a script.
        let page = r#"s.src = policy.createScriptURL('/s/_/kabuki/_/js/k\x3dkabuki.base\/m\x3dbase'); }}); } else {s.src = '/s/_/kabuki/_/js/k=kabuki.base/am=A/m=base';}s.id = 'base-js';"#;
        assert_eq!(
            base_js_path(page).as_deref(),
            Some("/s/_/kabuki/_/js/k=kabuki.base/am=A/m=base")
        );
        let js = r#"a.b={clientId:"12345-abc.apps.googleusercontent.com",Za:"SeCrEt_1"},c=2"#;
        assert_eq!(
            parse_client(js),
            Some(Client {
                id: "12345-abc.apps.googleusercontent.com".into(),
                secret: "SeCrEt_1".into()
            })
        );
        assert_eq!(parse_client(r#"clientId:"x"}"#), None);
    }

    #[test]
    fn code_and_tokens() {
        let c = parse_code(
            r#"{"device_code":"dc","user_code":"WZY-WHH-FZQ","expires_in":1800,"interval":5,"verification_url":"https://www.google.com/device"}"#,
        )
        .unwrap();
        assert_eq!((c.user_code.as_str(), c.interval), ("WZY-WHH-FZQ", 5));
        assert_eq!(
            parse_token(r#"{"error":"authorization_pending"}"#),
            TokenReply::Pending
        );
        assert_eq!(
            parse_token(r#"{"error":"slow_down"}"#),
            TokenReply::SlowDown
        );
        assert!(matches!(
            parse_token(r#"{"error":"expired_token"}"#),
            TokenReply::Failed(_)
        ));
        assert_eq!(
            parse_token(
                r#"{"access_token":"ya29.x","expires_in":3599,"refresh_token":"1//r","token_type":"Bearer"}"#
            ),
            TokenReply::Token {
                access: "ya29.x".into(),
                expires_in: 3599,
                refresh: Some("1//r".into())
            }
        );
        let cl = Client {
            id: "i".into(),
            secret: "s".into(),
        };
        assert!(
            poll_body(&cl, "dc")
                .contains(r#""grant_type":"http://oauth.net/grant_type/device/1.0""#)
        );
        assert!(refresh_body(&cl, "1//r").contains(r#""refresh_token":"1//r""#));
        assert!(code_body(&cl, "dev").contains(r#""device_model":"ytlr::""#));
    }

    #[test]
    fn accounts_round_trip_and_switch() {
        let mut a = Accounts {
            device_id: "dev".into(),
            ..Accounts::default()
        };
        a.add(Account {
            name: "Kat".into(),
            refresh_token: "1//a".into(),
            access_token: "not saved".into(),
            ..Account::default()
        });
        a.add(Account {
            name: "Tom".into(),
            refresh_token: "1//b".into(),
            ..Account::default()
        });
        assert_eq!(a.active, 1);
        let back = Accounts::from_conf(&a.to_conf());
        assert_eq!(back.list.len(), 2);
        assert_eq!(back.list[0].access_token, "");
        assert_eq!(back.active, 1);
        // The same account again replaces it.
        a.add(Account {
            name: "Kat".into(),
            refresh_token: "1//a2".into(),
            ..Account::default()
        });
        assert_eq!((a.list.len(), a.active), (2, 0));
        a.remove(0);
        assert_eq!((a.list.len(), a.active), (1, 0));
        assert!(!Account::default().fresh(0));
        assert!(
            Account {
                access_token: "t".into(),
                expires_at: 1000,
                ..Account::default()
            }
            .fresh(100)
        );
    }

    #[test]
    fn account_name() {
        let body = r#"{"contents":[{"accountSectionListRenderer":{"contents":[{"accountItemSectionRenderer":{"contents":[{"accountItem":{"accountName":{"simpleText":"Kat Cat"},"accountPhoto":{"thumbnails":[{"url":"https://yt3/a=s48"},{"url":"https://yt3/a=s88"}]}}}]}}]}}]}"#;
        assert_eq!(
            parse_account_name(body),
            Some(("Kat Cat".into(), "https://yt3/a=s88".into()))
        );
        assert_eq!(
            device_uuid("0123456789abcdef0123456789abcdef"),
            "01234567-89ab-4def-a123-456789abcdef"
        );
    }
}
