use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, File},
    future::Future,
    io::{BufReader, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

use reqwest::{Client, RequestBuilder, Response, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::runtime::{Builder, Runtime};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{APP_NAME, CancellationToken, Failure, Result};

mod album_sets;
pub use album_sets::AlbumSet;

// Public OAuth identifier for Tagcast Lite's own iBroadcast registration.
const CLIENT_ID: &str = "cbd63f6de5f511f1b50eb49691aa2236";
const SCOPES: &str = "user.library:read user.account:read";
const OAUTH: &str = "https://oauth.ibroadcast.com";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYCHAIN_SERVICE: &str = "nl.cyberdelia.tagcast-player";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const READ_TIMEOUT: Duration = Duration::from_secs(8);
const LIBRARY_READ_TIMEOUT: Duration = Duration::from_secs(120);
const LIBRARY_LIMIT: usize = 192 * 1024 * 1024;
const LIBRARY_FORMAT: u32 = 7;
const UPLOAD_METADATA_FORMAT: u32 = 4;
const OLDEST_LIBRARY_FORMAT: u32 = 3;
const ARTWORK_SERVER: &str = "https://artwork.ibroadcast.com";

#[derive(Clone)]
pub struct Http {
    pub client: Client,
    runtime: Arc<Runtime>,
}

impl Http {
    pub fn new() -> Result<Self> {
        let client = Self::client(READ_TIMEOUT)?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|_| Failure::Invalid("Could not start network worker"))?;
        Ok(Self {
            client,
            runtime: Arc::new(runtime),
        })
    }

    fn client(read_timeout: Duration) -> Result<Client> {
        Client::builder()
            .redirect(Policy::none())
            .user_agent(concat!("Tagcast-Lite/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(6))
            .read_timeout(read_timeout)
            .gzip(true)
            .build()
            .map_err(http_error)
    }

    pub fn run<T>(
        &self,
        stop: &CancellationToken,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        self.runtime.block_on(async {
            tokio::select! { biased;
                _ = stop.cancelled() => Err(Failure::Cancelled),
                result = future => result,
            }
        })
    }

    pub fn send(&self, stop: &CancellationToken, request: RequestBuilder) -> Result<Response> {
        self.run(stop, async { request.send().await.map_err(http_error) })
    }

    pub fn body(
        &self,
        stop: &CancellationToken,
        response: Response,
        max: usize,
        seconds: u64,
    ) -> Result<Vec<u8>> {
        self.run(stop, async {
            tokio::time::timeout(Duration::from_secs(seconds), async {
                let mut response = response;
                if response.content_length().is_some_and(|n| n > max as u64) {
                    return Err(Failure::Invalid("Response exceeds the size limit"));
                }
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                    if body.len() + chunk.len() > max {
                        return Err(Failure::Invalid("Response exceeds the size limit"));
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(body)
            })
            .await
            .map_err(|_| Failure::Network)?
        })
    }

    fn object<T: DeserializeOwned>(
        &self,
        stop: &CancellationToken,
        response: Response,
        max: usize,
    ) -> Result<T> {
        let seconds = if max <= 16_384 { 15 } else { 120 };
        serde_json::from_slice(&self.body(stop, response, max, seconds)?)
            .map_err(|_| Failure::Invalid("iBroadcast returned unreadable data"))
    }
}

pub fn http_error(error: reqwest::Error) -> Failure {
    // reqwest may include the signed URL in its Display implementation.
    let mut cause = error.source();
    while let Some(item) = cause {
        let text = item.to_string().to_ascii_lowercase();
        if ["certificate", "tls", "ssl error"]
            .iter()
            .any(|needle| text.contains(needle))
        {
            return Failure::Security;
        }
        cause = item.source();
    }
    if error.is_builder() {
        Failure::Invalid("Invalid iBroadcast request")
    } else {
        Failure::Network
    }
}

pub fn check_status(response: Response) -> Result<Response> {
    match response.status().as_u16() {
        200..=299 => Ok(response),
        401 | 403 => Err(Failure::Auth),
        status => Err(Failure::Http(status)),
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    expires: u64,
    #[serde(default)]
    owner: String,
}

#[derive(Deserialize)]
struct Grant {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default = "poll_interval")]
    interval: u64,
}

fn poll_interval() -> u64 {
    5
}

fn verification_url(address: &str) -> Result<crate::Url> {
    const INVALID: Failure =
        Failure::Invalid("iBroadcast returned an invalid device verification address");
    if address.bytes().any(|ch| ch <= b' ' || ch == b'\\') {
        return Err(INVALID);
    }
    // The device-code API currently sends the address without its scheme.
    let url = if address.starts_with("ibroadcast.com/")
        || address.starts_with("oauth.ibroadcast.com/")
        || address.starts_with("media.ibroadcast.com/")
    {
        crate::Url::parse(&format!("https://{address}"))
    } else {
        crate::Url::parse(address)
    }
    .map_err(|_| INVALID)?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("ibroadcast.com" | "oauth.ibroadcast.com" | "media.ibroadcast.com")
        )
        || !matches!(url.path(), "/device" | "/device/")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(INVALID);
    }
    Ok(url)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn grant(data: Grant, prior: &str) -> Result<Tokens> {
    let refresh = data.refresh_token.unwrap_or_else(|| prior.into());
    if data.access_token.is_empty() || refresh.is_empty() || data.expires_in == 0 {
        return Err(Failure::Auth);
    }
    Ok(Tokens {
        access: data.access_token,
        refresh,
        expires: now().saturating_add(data.expires_in),
        owner: String::new(),
    })
}

// Serialize only publication and session invalidation. Never hold this lock
// during requests or the serialization of a large library.
#[derive(Clone, Default)]
pub struct Session(Arc<Mutex<bool>>);

impl Session {
    pub fn close(&self) {
        *self.0.lock().unwrap() = true;
    }

    fn publish<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let closed = self.0.lock().unwrap();
        if *closed {
            return Err(Failure::Cancelled);
        }
        f()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LibraryStamp {
    owner: String,
    revision: String,
    combined: bool,
}

pub enum LibraryUpdate {
    Current { premium: bool },
    Replaced(Box<Library>),
}

pub struct Account {
    pub http: Http,
    client_id: String,
    tokens: Mutex<Tokens>,
    refresh_gate: Mutex<()>,
    session: Session,
}

impl Account {
    pub fn connect(
        http: Http,
        client_id: Option<String>,
        session: Session,
        stop: &CancellationToken,
        display: impl Fn(&str, &str),
    ) -> Result<Arc<Self>> {
        let client_id = client_id.unwrap_or_else(|| CLIENT_ID.into());
        let tokens = match load_tokens(&client_id)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| Failure::Invalid("Stored sign-in is unreadable. Sign out first"))?,
            None => {
                let tokens = Self::device_login(&http, &client_id, stop, display)?;
                if stop.is_cancelled() {
                    return Err(Failure::Cancelled);
                }
                session.publish(|| save_tokens(&client_id, &tokens))?;
                tokens
            }
        };
        Ok(Arc::new(Self {
            http,
            client_id,
            tokens: Mutex::new(tokens),
            refresh_gate: Mutex::new(()),
            session,
        }))
    }

    fn device_login(
        http: &Http,
        client_id: &str,
        stop: &CancellationToken,
        display: impl Fn(&str, &str),
    ) -> Result<Tokens> {
        let response = check_status(
            http.send(
                stop,
                http.client
                    .get(format!("{OAUTH}/device/code"))
                    .query(&[("client_id", client_id), ("scope", SCOPES)]),
            )?,
        )?;
        let code: DeviceCode = http.object(stop, response, 16_384)?;
        let verification = verification_url(&code.verification_uri)?;
        if code.user_code.is_empty()
            || !code
                .user_code
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(Failure::Invalid(
                "iBroadcast returned an invalid device code",
            ));
        }
        let complete = code
            .verification_uri_complete
            .as_deref()
            .and_then(|address| verification_url(address).ok())
            .filter(|url| {
                let mut values = url.query_pairs().filter(|(key, _)| key == "user_code");
                values
                    .next()
                    .is_some_and(|(_, value)| value == code.user_code)
                    && values.next().is_none()
            });
        display(
            complete.as_ref().unwrap_or(&verification).as_str(),
            &code.user_code,
        );
        let deadline = Instant::now() + Duration::from_secs(code.expires_in.min(1800));
        let mut interval = Duration::from_secs(code.interval.clamp(1, 60));
        while Instant::now() + interval < deadline {
            http.run(stop, async {
                tokio::time::sleep(interval).await;
                Ok(())
            })?;
            let response = http.send(
                stop,
                http.client.post(format!("{OAUTH}/token")).form(&[
                    ("grant_type", "device_code"),
                    ("client_id", client_id),
                    ("device_code", code.device_code.as_str()),
                ]),
            )?;
            let ok = response.status().is_success();
            let value: Value = http.object(stop, response, 16_384)?;
            if ok {
                return grant(
                    serde_json::from_value(value).map_err(|_| Failure::Auth)?,
                    "",
                );
            }
            match value["error"].as_str() {
                Some("authorization_pending") => {}
                Some("slow_down") => interval += Duration::from_secs(5),
                Some("access_denied") => {
                    return Err(Failure::Invalid("iBroadcast sign-in was declined"));
                }
                Some("expired_token") => {
                    return Err(Failure::Invalid(
                        "The device code expired. Start sign-in again",
                    ));
                }
                _ => {
                    return Err(Failure::Invalid(
                        "Could not complete the device-code sign-in",
                    ));
                }
            }
        }
        Err(Failure::Invalid(
            "The device code expired. Start sign-in again",
        ))
    }

    fn renew(&self, used: &Tokens, stop: &CancellationToken) -> Result<Tokens> {
        let _gate = self.refresh_gate.lock().unwrap();
        let current = self.tokens.lock().unwrap().clone();
        if stop.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        self.session.publish(|| Ok(()))?;
        if current.access != used.access
            || current.refresh != used.refresh
            || current.expires != used.expires
        {
            return Ok(current);
        }
        // Once the server can rotate a refresh token, keep the response even
        // if the player skips or stops the track that needed the renewal.
        let renewal = CancellationToken::new();
        let response = self.http.send(
            &renewal,
            self.http.client.post(format!("{OAUTH}/token")).form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("refresh_token", current.refresh.as_str()),
            ]),
        )?;
        let status = response.status().as_u16();
        let data: Value = self.http.object(&renewal, response, 16_384)?;
        if status != 200 {
            return if matches!(
                data["error"].as_str(),
                Some("invalid_grant" | "invalid_client" | "unauthorized_client")
            ) {
                Err(Failure::Auth)
            } else {
                Err(Failure::Http(status))
            };
        }
        let mut next = grant(
            serde_json::from_value(data).map_err(|_| Failure::Auth)?,
            &current.refresh,
        )?;
        self.session.publish(|| {
            let mut tokens = self.tokens.lock().unwrap();
            next.owner.clone_from(&tokens.owner);
            *tokens = next.clone();
            save_tokens(&self.client_id, &next)?;
            Ok(next)
        })
    }

    pub fn authorized<T>(
        &self,
        stop: &CancellationToken,
        send: impl Fn(&str) -> Result<T>,
    ) -> Result<T> {
        self.session.publish(|| Ok(()))?;
        let current = self.tokens.lock().unwrap().clone();
        let current = if now() >= current.expires {
            self.renew(&current, stop)?
        } else {
            current
        };
        match send(&current.access) {
            Err(Failure::Auth) => {
                let renewed = self.renew(&current, stop)?;
                send(&renewed.access)
            }
            other => other,
        }
    }

    fn read_api(&self, stop: &CancellationToken, mode: &str) -> Result<Value> {
        // The large library response can take longer to generate than an
        // ordinary API request. Keep the short timeout on playback requests.
        let library_client = (mode == "library")
            .then(|| Http::client(LIBRARY_READ_TIMEOUT))
            .transpose()?;
        let (client, url, limit) = match &library_client {
            Some(client) => (client, "https://library.ibroadcast.com", LIBRARY_LIMIT),
            None => (
                &self.http.client,
                "https://api.ibroadcast.com/status",
                4 * 1024 * 1024,
            ),
        };
        self.authorized(stop, |token| {
            let response = check_status(self.http.send(stop, client.post(url).bearer_auth(token)
                .json(&json!({"mode": mode, "client": "tagcast-player", "version": VERSION, "device_name": APP_NAME, "user_agent": concat!("Tagcast-Lite/", env!("CARGO_PKG_VERSION"))})))?)?;
            let result: Value = self.http.object(stop, response, limit)?;
            if result["authenticated"] == false { return Err(Failure::Auth); }
            if result["result"] == false { return Err(Failure::Invalid("iBroadcast rejected the read request")); }
            Ok(result)
        })
    }

    pub fn cached_library(&self) -> Option<Library> {
        let owner = self.tokens.lock().unwrap().owner.clone();
        if owner.is_empty() {
            return None;
        }
        let mut library = read_cache(&self.client_id)?;
        if library.owner != owner {
            return None;
        }
        library.prepare();
        Some(library)
    }

    pub fn library(
        &self,
        stop: &CancellationToken,
        known: Option<LibraryStamp>,
    ) -> Result<LibraryUpdate> {
        let status = self.read_api(stop, "status")?;
        let user = &status["user"];
        let mut owner = scalar(&user["id"]);
        if owner.is_empty() {
            owner = scalar(&user["user_id"]);
        }
        if owner.parse::<u64>().ok().filter(|id| *id > 0).is_none() {
            return Err(Failure::Invalid("iBroadcast did not identify the account"));
        }
        let stamp = LibraryStamp {
            owner,
            revision: scalar(&status["status"]["lastmodified"]),
            combined: truth(&user["preferences"]["combine_sets"]),
        };
        let premium = truth(&user["premium"]);
        // A verified status belongs to the same authenticated connection. Bind
        // future offline metadata to that identity without replacing credentials.
        self.session.publish(|| {
            let mut tokens = self.tokens.lock().unwrap();
            if tokens.owner != stamp.owner {
                tokens.owner.clone_from(&stamp.owner);
                save_tokens(&self.client_id, &tokens)?;
            }
            Ok(())
        })?;
        if !stamp.revision.is_empty() && known.as_ref() == Some(&stamp) {
            return Ok(LibraryUpdate::Current { premium });
        }
        let data = self.read_api(stop, "library")?;
        let mut library = Library::parse_view(&data, stamp.combined)?;
        if library.artwork_server == ARTWORK_SERVER {
            library.artwork_server = artwork_server(status["settings"]["artwork_server"].as_str());
        }
        library.owner = stamp.owner;
        library.revision = stamp.revision;
        library.premium = premium;
        if stop.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        let _ = write_cache(self, &library);
        self.session
            .publish(|| Ok(LibraryUpdate::Replaced(Box::new(library))))
    }
}

fn save_tokens(client_id: &str, tokens: &Tokens) -> Result<()> {
    let encoded =
        serde_json::to_vec(tokens).map_err(|_| Failure::Invalid("Could not encode the sign-in"))?;
    store_tokens(client_id, &encoded)
}

#[cfg(target_os = "macos")]
fn load_tokens(client_id: &str) -> Result<Option<Vec<u8>>> {
    match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, client_id) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(_) => Err(Failure::Invalid("Could not read the macOS Keychain")),
    }
}

#[cfg(target_os = "macos")]
fn store_tokens(client_id: &str, encoded: &[u8]) -> Result<()> {
    security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, client_id, encoded)
        .map_err(|_| Failure::Invalid("Could not save the sign-in in the macOS Keychain"))
}

#[cfg(target_os = "macos")]
fn forget_tokens(client_id: &str) -> Result<()> {
    match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, client_id) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(_) => Err(Failure::Invalid(
            "Could not remove the sign-in from the macOS Keychain",
        )),
    }
}

// Outside macOS the sign-in lives in a private file next to the library snapshot.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn token_path(client_id: &str) -> Result<PathBuf> {
    private_file(client_id, "credentials").ok_or(Failure::Invalid(
        "Could not find a private folder for the sign-in",
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn load_tokens(client_id: &str) -> Result<Option<Vec<u8>>> {
    match fs::read(token_path(client_id)?) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Failure::Invalid("Could not read the stored sign-in")),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn store_tokens(client_id: &str, encoded: &[u8]) -> Result<()> {
    write_private(&token_path(client_id)?, |file| file.write_all(encoded))
        .map_err(|_| Failure::Invalid("Could not save the sign-in"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn forget_tokens(client_id: &str) -> Result<()> {
    match fs::remove_file(token_path(client_id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Failure::Invalid("Could not remove the stored sign-in")),
    }
}

#[cfg(target_os = "windows")]
fn credential(client: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, client)
        .map_err(|_| Failure::Invalid("Could not open Windows Credential Manager"))
}

#[cfg(target_os = "windows")]
fn load_tokens(client: &str) -> Result<Option<Vec<u8>>> {
    match credential(client)?.get_secret() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(Failure::Invalid(
            "Could not read Windows Credential Manager",
        )),
    }
}

#[cfg(target_os = "windows")]
fn store_tokens(client: &str, encoded: &[u8]) -> Result<()> {
    credential(client)?
        .set_secret(encoded)
        .map_err(|_| Failure::Invalid("Could not save the Windows sign-in"))
}

#[cfg(target_os = "windows")]
fn forget_tokens(client: &str) -> Result<()> {
    match credential(client)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(Failure::Invalid("Could not remove the Windows sign-in")),
    }
}

pub fn logout(client_id: Option<String>) -> Result<()> {
    let client_id = client_id.unwrap_or_else(|| CLIENT_ID.into());
    forget_tokens(&client_id)?;
    if let Some(path) = cache_path(&client_id) {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(Failure::Invalid(
                    "Could not remove the local library snapshot",
                ));
            }
        }
    }
    Ok(())
}

/// The folder for the player's private files: the library snapshot and,
/// outside macOS, the sign-in.
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("Tagcast Player"));
    }
    let home = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?);
    if cfg!(target_os = "macos") {
        return Some(home.join("Library/Application Support/Tagcast Player"));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    Some(base.join("tagcast-player"))
}

fn private_file(client: &str, kind: &str) -> Option<PathBuf> {
    if client.is_empty()
        || client.len() > 128
        || !client
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    Some(data_dir()?.join(format!("{kind}-{client}.json")))
}

fn cache_path(client: &str) -> Option<PathBuf> {
    private_file(client, "library")
}

fn stage_private(
    path: &std::path::Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> std::io::Result<tempfile::NamedTempFile> {
    let folder = path.parent().unwrap();
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(folder)?;
    let mut temp = tempfile::NamedTempFile::new_in(folder)?;
    #[cfg(unix)]
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    write(temp.as_file_mut())?;
    temp.as_file().sync_all()?;
    Ok(temp)
}

pub fn write_private(
    path: &std::path::Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    stage_private(path, write)?.persist(path)?;
    Ok(())
}

/// Publishes a private file atomically if another process has not created it.
pub fn write_private_once(
    path: &std::path::Path,
    write: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match stage_private(path, write)?.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error),
    }
}

fn read_cache(client: &str) -> Option<Library> {
    let file = File::open(cache_path(client)?).ok()?;
    if file.metadata().ok()?.len() > LIBRARY_LIMIT as u64 {
        return None;
    }
    let library: Library = serde_json::from_reader(BufReader::new(file)).ok()?;
    (OLDEST_LIBRARY_FORMAT..=LIBRARY_FORMAT)
        .contains(&library.format)
        .then_some(library)
}

fn write_cache(account: &Account, library: &Library) -> Result<()> {
    let failure = |_| Failure::Invalid("Could not save the library snapshot");
    let Some(path) = cache_path(&account.client_id) else {
        return Ok(());
    };
    let temp = stage_private(&path, |file| {
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, library)?;
        writer.flush()
    })
    .map_err(failure)?;
    account.session.publish(|| {
        temp.persist(path)
            .map_err(|_| Failure::Invalid("Could not save the library snapshot"))?;
        Ok(())
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: u64,
    pub disc: u64,
    pub number: u64,
    pub seconds: f64,
    pub path: String,
    pub mime: String,
    #[serde(default)]
    pub artwork: u64,
    #[serde(default)]
    pub year: u32,
    #[serde(default)]
    pub album_artist: String,
    #[serde(skip)]
    pub search: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u64,
    pub title: String,
    pub tracks: Vec<u64>,
    pub folder: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct AlbumMetadata {
    pub added: u64,
    pub rating: u8,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    format: u32,
    pub owner: String,
    pub premium: bool,
    pub expires: String,
    #[serde(default)]
    artwork_server: String,
    revision: String,
    combined: bool,
    pub tracks: Vec<Arc<Track>>,
    pub playlists: Vec<Playlist>,
    pub genres: std::collections::BTreeMap<String, Vec<u64>>,
    pub tags: std::collections::BTreeMap<String, Vec<u64>>,
    pub favourite_tracks: Vec<u64>,
    pub favourite_albums: Vec<u64>,
    #[serde(default)]
    pub album_metadata: std::collections::BTreeMap<u64, AlbumMetadata>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub album_sets: std::collections::BTreeMap<u64, AlbumSet>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub artist_portraits: std::collections::BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub album_artist_ids: BTreeMap<u64, Vec<u64>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_performer_ids: BTreeMap<u64, Vec<u64>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    artist_names: BTreeMap<u64, String>,
}

fn entry<'a>(row: &'a Value, table: &Value, key: &str) -> &'a Value {
    if row.is_array()
        && let Some(index) = table["map"][key].as_u64()
    {
        &row[index as usize]
    } else {
        &row[key]
    }
}

fn additional_artists<'a>(
    row: &'a Value,
    table: &'a Value,
) -> impl Iterator<Item = (u64, &'a Value)> {
    let fields = &table["map"]["artists_additional_map"];
    entry(row, table, "artists_additional")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(move |credit| {
            let field = |name: &str, default: usize| {
                if let Some(values) = credit.as_array() {
                    let index = fields[name]
                        .as_u64()
                        .map(|index| index as usize)
                        .or_else(|| fields.is_null().then_some(default));
                    index
                        .and_then(|index| values.get(index))
                        .unwrap_or(&Value::Null)
                } else {
                    &credit[name]
                }
            };
            let id = number(field("artist_id", 0));
            (id > 0).then(|| (id, field("type", 2)))
        })
}

pub fn fold(text: &str) -> String {
    text.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(1024)
        .collect()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap_or(0)
}

fn date_component(value: &str) -> Option<u64> {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse().ok())?
}

fn upload_order(day: &str, clock: &str) -> Option<u64> {
    let day = day.trim();
    if day.get(4..5) != Some("-") || day.get(7..8) != Some("-") {
        return None;
    }
    let year = date_component(day.get(..4)?)?;
    let month = date_component(day.get(5..7)?)?;
    let day_of_month = date_component(day.get(8..10)?)?;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            28 + u64::from(
                year.is_multiple_of(400) || year.is_multiple_of(4) && !year.is_multiple_of(100),
            )
        }
        _ => return None,
    };
    if year == 0 || !(1..=days).contains(&day_of_month) {
        return None;
    }
    let date = year * 10_000 + month * 100 + day_of_month;
    let clock = clock.trim();
    let clock = if clock.is_empty() {
        day.get(11..19).unwrap_or_default()
    } else {
        clock
    };
    let time = (|| {
        if clock.len() != 8 || clock.get(2..3) != Some(":") || clock.get(5..6) != Some(":") {
            return None;
        }
        let hour = date_component(clock.get(..2)?)?;
        let minute = date_component(clock.get(3..5)?)?;
        let second = date_component(clock.get(6..8)?)?;
        (hour < 24 && minute < 60 && second < 60)
            .then_some(hour * 10_000 + minute * 100 + second + 1)
    })();
    Some(date * 1_000_000 + time.unwrap_or(0))
}

fn truth(value: &Value) -> bool {
    *value == true
        || number(value) == 1
        || value
            .as_str()
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Only an HTTPS iBroadcast host may serve cover art.
fn artwork_server(value: Option<&str>) -> String {
    value
        .and_then(|text| crate::Url::parse(text.trim_end_matches('/')).ok())
        .filter(|url| {
            url.scheme() == "https"
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.host_str().is_some_and(|host| {
                    host == "ibroadcast.com" || host.ends_with(".ibroadcast.com")
                })
        })
        .map(|url| url.as_str().trim_end_matches('/').to_owned())
        .unwrap_or_else(|| ARTWORK_SERVER.into())
}

impl Library {
    pub fn has_album_metadata(&self) -> bool {
        self.format >= UPLOAD_METADATA_FORMAT
    }

    pub fn artist_name(&self, id: u64) -> &str {
        if id == 0 {
            "Various Artists"
        } else {
            self.artist_names
                .get(&id)
                .map_or("Unknown artist", String::as_str)
        }
    }

    pub fn stamp(&self) -> LibraryStamp {
        LibraryStamp {
            owner: self.owner.clone(),
            // Older snapshots remain available while new display metadata is fetched.
            revision: if self.format == LIBRARY_FORMAT {
                self.revision.clone()
            } else {
                String::new()
            },
            combined: self.combined,
        }
    }

    /// The cover image address for an iBroadcast artwork id, at 150, 300 or 1000 pixels.
    pub fn artwork_url(&self, artwork: u64, size: u32) -> Option<String> {
        let server = if self.artwork_server.is_empty() {
            ARTWORK_SERVER
        } else {
            &self.artwork_server
        };
        (artwork > 0).then(|| format!("{server}/artwork/{artwork}-{size}"))
    }

    /// Test fixtures may serve cover art from the loopback address.
    #[doc(hidden)]
    pub fn fixture_artwork_server(&mut self, server: &str) {
        if let Ok(url) = crate::Url::parse(server)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
        {
            self.artwork_server = server.trim_end_matches('/').into();
        }
    }

    pub fn prepare(&mut self) {
        for row in &mut self.tracks {
            let track = Arc::make_mut(row);
            track.search = fold(&format!(
                "{} {} {} {}",
                track.title, track.artist, track.album, track.album_artist
            ));
        }
    }

    pub fn parse(value: &Value) -> Result<Self> {
        Self::parse_view(value, truth(&value["combine_sets"]))
    }

    fn parse_view(value: &Value, combined: bool) -> Result<Self> {
        let data = &value["library"];
        let (tracks, albums, artists, playlists) = (
            &data["tracks"],
            &data["albums"],
            &data["artists"],
            &data["playlists"],
        );
        let rows = tracks
            .as_object()
            .filter(|_| albums.is_object() && artists.is_object())
            .ok_or(Failure::Invalid(
                "iBroadcast returned an incomplete library",
            ))?;
        let mut result = Self {
            format: LIBRARY_FORMAT,
            combined,
            expires: scalar(&data["expires"]),
            artwork_server: artwork_server(value["settings"]["artwork_server"].as_str()),
            artist_portraits: artist_portraits(artists),
            ..Self::default()
        };
        let mut layout = Vec::new();
        for (id, row) in rows {
            let Ok(id) = id.parse::<u64>() else {
                continue;
            };
            let field = |key| entry(row, tracks, key);
            let path = field("file").as_str().unwrap_or_default();
            if id == 0 || truth(field("trashed")) || !crate::transport::valid_track_path(path) {
                continue;
            }
            let album_id = number(field("album_id"));
            let album = &albums[album_id.to_string()];
            let artist_id = number(field("artist_id"));
            let artist = &artists[artist_id.to_string()];
            let title = clean(field("title").as_str().unwrap_or("Unknown track"));
            let artist = clean(
                entry(artist, artists, "name")
                    .as_str()
                    .or_else(|| field("artist").as_str())
                    .unwrap_or(""),
            );
            let album_name = clean(entry(album, albums, "name").as_str().unwrap_or(""));
            let album_artist_id = number(entry(album, albums, "artist_id"));
            let album_artist = if album_artist_id == 0 {
                String::new()
            } else {
                let row = &artists[album_artist_id.to_string()];
                clean(entry(row, artists, "name").as_str().unwrap_or(""))
            };
            let year = [number(field("year")), number(entry(album, albums, "year"))]
                .into_iter()
                .find(|year| (1..=9999).contains(year))
                .unwrap_or(0) as u32;
            let upload_day = field("uploaded_on").as_str().unwrap_or_default().trim();
            let added = upload_order(
                upload_day,
                field("uploaded_time").as_str().unwrap_or_default(),
            )
            .unwrap_or(0);
            if added > 0 {
                let meta = result.album_metadata.entry(album_id).or_default();
                meta.added = meta.added.max(added);
            }
            let primary = std::iter::once(field("genre"));
            let additional = field("genres_additional").as_array().into_iter().flatten();
            for label in primary.chain(additional).filter_map(Value::as_str).take(32) {
                let label = clean(label);
                if !label.trim().is_empty() {
                    let ids = result.genres.entry(label).or_default();
                    if ids.last() != Some(&id) {
                        ids.push(id);
                    }
                }
            }
            if number(field("rating")) == 5 {
                result.favourite_tracks.push(id);
            }
            let mut performers = Vec::new();
            for (performer, role) in additional_artists(row, tracks) {
                if performer != artist_id
                    && (role.is_null() || matches!(role.as_str(), Some("" | "artist" | "band")))
                    && !performers.contains(&performer)
                {
                    performers.push(performer);
                }
            }
            if !performers.is_empty() {
                result.extra_performer_ids.insert(id, performers);
            }
            let raw_number = field("track");
            let disc = number(entry(album, albums, "disc"));
            let (disc, track_number) = album_sets::explicit_position(raw_number)
                .unwrap_or_else(|| (disc, number(raw_number)));
            layout.push(album_sets::TrackLayout {
                day: upload_day.get(..10).unwrap_or(upload_day),
                added,
                numeric: raw_number.is_u64(),
            });
            result.tracks.push(Arc::new(Track {
                id,
                title,
                artist,
                album: album_name,
                album_id,
                path: path.into(),
                disc,
                number: track_number,
                seconds: field("length")
                    .as_f64()
                    .or_else(|| field("length").as_str()?.parse().ok())
                    .filter(|x| x.is_finite() && *x > 0.0 && *x <= 86400.0)
                    .unwrap_or(0.0),
                mime: field("type").as_str().unwrap_or_default().into(),
                artwork: number(field("artwork_id")),
                year,
                album_artist,
                search: String::new(),
            }));
        }
        result.album_sets = album_sets::build(albums, artists, &result.tracks, &layout, combined);
        result.prepare();
        result
            .tracks
            .sort_by_cached_key(|t| (fold(&t.artist), fold(&t.album), t.disc, t.number, t.id));
        if let Some(rows) = playlists.as_object() {
            for (id, row) in rows {
                let Ok(id) = id.parse::<u64>() else {
                    continue;
                };
                if id == 0 || truth(entry(row, playlists, "trashed")) {
                    continue;
                }
                if let Some(ids) = entry(row, playlists, "tracks").as_array() {
                    result.playlists.push(Playlist {
                        id,
                        folder: entry(row, playlists, "type") == "folder"
                            && !truth(entry(row, playlists, "system_created")),
                        title: clean(entry(row, playlists, "name").as_str().unwrap_or("Playlist")),
                        tracks: ids.iter().map(number).filter(|id| *id > 0).collect(),
                    });
                }
            }
            result.playlists.sort_by_cached_key(|p| fold(&p.title));
        }
        if let Some(rows) = albums.as_object() {
            for (id, row) in rows {
                let Ok(id) = id.parse::<u64>() else { continue };
                if truth(entry(row, albums, "trashed")) {
                    continue;
                }
                let native_primary = entry(row, albums, "artist_id");
                let mut additional = additional_artists(row, albums).peekable();
                if !native_primary.is_null() || additional.peek().is_some() {
                    let mut credits = vec![number(native_primary)];
                    for (artist, _) in additional {
                        if !credits.contains(&artist) {
                            credits.push(artist);
                        }
                    }
                    result.album_artist_ids.insert(id, credits);
                }
                let rating = number(entry(row, albums, "rating"));
                if (1..=5).contains(&rating) {
                    result.album_metadata.entry(id).or_default().rating = rating as u8;
                    if rating == 5 {
                        result.favourite_albums.push(id);
                    }
                }
            }
        }
        let credit_ids: BTreeSet<_> = result
            .album_artist_ids
            .values()
            .chain(result.extra_performer_ids.values())
            .flatten()
            .copied()
            .collect();
        for id in credit_ids.into_iter().filter(|id| *id != 0) {
            if let Some(name) = entry(&artists[id.to_string()], artists, "name").as_str() {
                let name = clean(name);
                if !name.trim().is_empty() {
                    result.artist_names.insert(id, name);
                }
            }
        }
        if let Some(rows) = data["tags"].as_object() {
            for row in rows.values() {
                if truth(row.get("archived").unwrap_or(&Value::Null)) {
                    continue;
                }
                let (Some(label), Some(tracks)) = (row["name"].as_str(), row["tracks"].as_array())
                else {
                    continue;
                };
                let label = clean(label);
                if label.trim().is_empty() {
                    continue;
                }
                result
                    .tags
                    .entry(label)
                    .or_default()
                    .extend(tracks.iter().map(number).filter(|id| *id > 0));
            }
        }
        Ok(result)
    }
}

fn artist_portraits(artists: &Value) -> std::collections::BTreeMap<String, u64> {
    let mut portraits = std::collections::BTreeMap::new();
    for (id, row) in artists.as_object().into_iter().flatten() {
        if !id.parse::<u64>().is_ok_and(|id| id > 0) {
            continue;
        }
        let artwork = number(entry(row, artists, "artwork_id"));
        let name = clean(entry(row, artists, "name").as_str().unwrap_or_default());
        if artwork != 0 && !name.trim().is_empty() {
            // The artist browser merges matching name keys. An ambiguous
            // portrait falls back to a cover from the artist's albums.
            portraits
                .entry(fold(name.trim()))
                .and_modify(|stored| {
                    if *stored != artwork {
                        *stored = 0;
                    }
                })
                .or_insert(artwork);
        }
    }
    portraits.retain(|_, artwork| *artwork > 0);
    portraits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_session_cannot_publish_after_sign_out() {
        let session = Session::default();
        let worker = session.clone();
        assert!(worker.publish(|| Ok(1)).is_ok());
        session.close();
        assert_eq!(
            worker.publish(|| panic!("a retired worker published data")),
            Err::<(), _>(Failure::Cancelled)
        );
    }

    #[test]
    fn normalizes_the_live_device_address_and_accepts_the_https_form() {
        for (input, expected) in [
            ("ibroadcast.com/device", "https://ibroadcast.com/device"),
            (
                "oauth.ibroadcast.com/device",
                "https://oauth.ibroadcast.com/device",
            ),
            (
                "https://oauth.ibroadcast.com/device",
                "https://oauth.ibroadcast.com/device",
            ),
        ] {
            assert_eq!(verification_url(input).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn device_address_requires_a_trusted_https_origin_and_route() {
        for input in [
            "http://ibroadcast.com/device",
            "ibroadcast.com.example/device",
            "https://example.com/device",
            "https://ibroadcast.com@example.com/device",
            "https://ignored@ibroadcast.com/device",
            "https://ibroadcast.com:444/device",
            "https://ibroadcast.com/device#unexpected",
            "https://ibroadcast.com/other",
            "//ibroadcast.com/device",
        ] {
            assert!(verification_url(input).is_err(), "{input}");
        }
    }

    #[test]
    #[ignore = "requests an unapproved device code from the public iBroadcast OAuth endpoint"]
    fn live_device_login_reaches_browser_handoff() {
        let http = Http::new().unwrap();
        let stop = CancellationToken::new();
        let displayed = std::cell::Cell::new(false);
        let result = Account::device_login(&http, CLIENT_ID, &stop, |address, _| {
            assert!(verification_url(address).is_ok());
            displayed.set(true);
            stop.cancel();
        });
        assert!(
            displayed.get(),
            "Device-code flow did not reach the browser handoff"
        );
        assert!(matches!(result, Err(Failure::Cancelled)));
    }

    #[test]
    fn library_keeps_tracks_and_ordered_playlist_ids_across_row_shapes() {
        let library = Library::parse(&json!({
            "library": {
                "expires": 1234,
                "artists": {"map": {"name": 0}, "41": ["Björk"]},
                "albums": {"map": {"name": 0, "disc": 1}, "72": ["Debut", 1]},
                "tracks": {
                    "map": {"title": 0, "artist_id": 1, "album_id": 2, "file": 3, "track": 4, "trashed": 5},
                    "10": ["Venus as a Boy", 41, 72, "/128/abc/10", 2, false],
                    "20": {"title": "Human Behaviour", "artist_id": 41, "album_id": 72, "file": "/orig/abc/20", "track": 1},
                    "30": ["Trashed", 41, 72, "/128/abc/30", 3, true],
                    "40": ["Invalid", 41, 72, "/128/../40", 4, false]
                },
                "playlists": {"map": {"name": 0, "tracks": 1}, "55": ["Set", [10, "20", 10]]}
            }
        })).unwrap();
        assert_eq!(library.expires, "1234");
        assert_eq!(
            library.tracks.iter().map(|t| t.id).collect::<Vec<_>>(),
            [20, 10]
        );
        assert!(library.tracks[0].search.contains("bjork"));
        assert_eq!(library.playlists[0].tracks, [10, 20, 10]);
    }

    #[test]
    fn library_reads_artwork_year_and_album_artist() {
        let library = Library::parse(&json!({
            "settings": {"artwork_server": "https://artwork.ibroadcast.com/"},
            "library": {
                "artists": {"map": {"name": 0}, "1": ["Massive Attack"], "2": ["Horace Andy"]},
                "albums": {"map": {"name": 0, "artist_id": 1, "year": 2}, "9": ["Mezzanine", 1, 1998]},
                "tracks": {
                    "map": {"title": 0, "artist_id": 1, "album_id": 2, "file": 3, "artwork_id": 4, "year": 5},
                    "5": ["Angel", 2, 9, "/128/x/5", 777, 0]
                }
            }
        }))
        .unwrap();
        let track = &library.tracks[0];
        assert_eq!((track.artwork, track.year), (777, 1998));
        assert_eq!(track.album_artist, "Massive Attack");
        assert_eq!(
            library.artwork_url(track.artwork, 300).as_deref(),
            Some("https://artwork.ibroadcast.com/artwork/777-300")
        );
        assert_eq!(library.artwork_url(0, 300), None);
    }

    #[test]
    fn artwork_server_must_be_an_https_ibroadcast_host() {
        assert_eq!(
            artwork_server(Some("https://art2.ibroadcast.com/")),
            "https://art2.ibroadcast.com"
        );
        for bad in [
            "http://artwork.ibroadcast.com",
            "https://ibroadcast.com.example.org",
            "https://example.com",
            "https://u@artwork.ibroadcast.com",
            "https://artwork.ibroadcast.com:8443",
        ] {
            assert_eq!(artwork_server(Some(bad)), ARTWORK_SERVER, "{bad}");
        }
    }

    #[test]
    fn upload_order_accepts_real_dates_and_retains_available_precision() {
        let day = upload_order("2024-02-29", "").unwrap();
        let midnight = upload_order("2024-02-29T00:00:00", "").unwrap();
        let evening = upload_order("2024-02-29", "21:42:09").unwrap();
        assert!(day < midnight && midnight < evening);
        assert!(evening < upload_order("2024-03-01", "").unwrap());
        assert_eq!(upload_order("2024-02-29", "24:00:00"), Some(day));
        assert_eq!(upload_order("2023-02-29", ""), None);
        assert_eq!(upload_order("2024-04-31", ""), None);
        assert_eq!(upload_order("2024-０1-01", ""), None);
    }

    #[test]
    fn previous_cache_can_be_displayed_but_forces_metadata_refresh() {
        let current = Library {
            format: LIBRARY_FORMAT,
            owner: "73".into(),
            revision: "4321".into(),
            ..Library::default()
        };
        let mut previous = serde_json::to_value(&current).unwrap();
        previous["format"] = json!(OLDEST_LIBRARY_FORMAT);
        previous.as_object_mut().unwrap().remove("album_metadata");
        let parsed: Library = serde_json::from_value(previous).unwrap();
        assert_eq!(current.stamp().revision, "4321");
        assert_eq!(parsed.stamp().owner, "73");
        assert!(parsed.stamp().revision.is_empty());
        assert!(parsed.album_metadata.is_empty());
        assert!(!parsed.has_album_metadata());
        let mut v4 = current;
        v4.format = UPLOAD_METADATA_FORMAT;
        assert!(v4.has_album_metadata());
        assert!(v4.stamp().revision.is_empty());
    }

    #[test]
    fn artist_portraits_read_compact_and_named_rows_and_omit_ambiguous_names() {
        let artists = json!({
            "map": {"name":0, "artwork_id":1},
            "1": ["Björk", 40],
            "2": {"name":"The Cure", "artwork_id":50},
            "3": ["Guest", 60],
            "4": ["Güest", 70],
            "5": ["  ", 80],
            "6": ["Album cover only", 0],
            "0": ["Reserved", 1]
        });
        let portraits = artist_portraits(&artists);
        assert_eq!(portraits.len(), 2);
        assert_eq!(portraits["bjork"], 40);
        assert_eq!(portraits["the cure"], 50);
    }
}
