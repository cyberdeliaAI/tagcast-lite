//! Public stable-release checks and explicitly requested updates, independent of playback and sign-in.
mod install;

use eframe::egui;
use reqwest::{Client, redirect::Policy};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Write,
    sync::mpsc::{self, Receiver},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tagcast_player::CancellationToken;
use tempfile::TempDir;

const REPOSITORY: &str = "https://github.com/cyberdeliaAI/tagcast-lite";
const API: &str = "https://api.github.com/repos/cyberdeliaAI/tagcast-lite/releases/latest";
const DAY: u64 = 24 * 60 * 60;
const LIMIT: u64 = 64 * 1024 * 1024;
const CURRENT: &str = env!("CARGO_PKG_VERSION");
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Release {
    pub version: String,
    pub notes: String,
    asset: Option<Asset>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: String,
}
#[derive(Deserialize)]
struct Metadata {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<GitAsset>,
}
#[derive(Deserialize)]
struct GitAsset {
    name: String,
    size: u64,
    digest: Option<String>,
}

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-arm64.zip"),
        ("macos", "x86_64") => Some("macos-x64.zip"),
        ("windows", "x86_64") => Some("windows-x64.zip"),
        ("linux", "x86_64") => Some("linux-x64.tar.gz"),
        _ => None,
    }
}
fn stable(s: &str) -> Option<Version> {
    let v = Version::parse(s).ok()?;
    (v.pre.is_empty() && v.build.is_empty()).then_some(v)
}
impl Release {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let m: Metadata =
            serde_json::from_slice(bytes).map_err(|_| "Unreadable GitHub release information")?;
        let version = m
            .tag_name
            .strip_prefix('v')
            .filter(|s| stable(s).is_some())
            .ok_or("Invalid release version")?;
        if m.draft || m.prerelease {
            return Err("GitHub did not return a stable release".into());
        }
        let asset = platform().and_then(|suffix| {
            let name = format!("tagcast-lite-{version}-{suffix}");
            let mut matching = m.assets.into_iter().filter(|a| a.name == name);
            let a = matching.next()?;
            if matching.next().is_some() || !(1..=LIMIT).contains(&a.size) {
                return None;
            }
            let digest = a.digest?.strip_prefix("sha256:")?.to_ascii_lowercase();
            (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(Asset {
                name,
                size: a.size,
                digest,
            })
        });
        Ok(Self {
            version: version.into(),
            notes: m.body.unwrap_or_default().chars().take(24_000).collect(),
            asset,
        })
    }
    fn newer_than(&self, current: &str) -> bool {
        stable(&self.version)
            .zip(Version::parse(current).ok())
            .is_some_and(|(next, now)| next > now)
    }
}

fn client() -> Result<Client> {
    Client::builder()
        .no_proxy()
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 4
                && url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some(
                        "api.github.com"
                            | "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(concat!("Tagcast-Lite/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "Could not start the update connection".into())
}
async fn metadata(client: &Client) -> Result<Release> {
    let mut res = client
        .get(API)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| "Could not reach GitHub")?;
    if !res.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", res.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|_| "The release check was interrupted")?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("Release information is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Release::parse(&bytes)
}
async fn download(
    client: &Client,
    release: &Release,
    asset: &Asset,
    file: &mut File,
) -> Result<()> {
    let url = format!(
        "{REPOSITORY}/releases/download/v{}/{}",
        release.version, asset.name
    );
    let response = client
        .get(url)
        .timeout(Duration::from_secs(160))
        .send()
        .await
        .map_err(|_| "Could not download the update")?;
    save_verified(response, asset, file).await
}

async fn save_verified(
    mut response: reqwest::Response,
    asset: &Asset,
    file: &mut File,
) -> Result<()> {
    if response.status() != 200 {
        return Err(format!(
            "Download returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut hash = Sha256::new();
    let mut total = 0;
    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|_| "The download was interrupted")?
    {
        total += bytes.len() as u64;
        if total > asset.size {
            return Err("The archive exceeds its published size".into());
        }
        hash.update(&bytes);
        file.write_all(&bytes)
            .map_err(|_| "Could not write the downloaded archive")?;
    }
    if total != asset.size || format!("{:x}", hash.finalize()) != asset.digest {
        return Err("The downloaded archive failed its SHA-256 check".into());
    }
    file.sync_all()
        .map_err(|_| "Could not finish writing the archive".into())
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Cache {
    attempted: u64,
    latest: Option<Release>,
    failed: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Phase {
    Idle,
    Checking,
    Downloading,
    Ready,
    Installing,
}
struct Prepared {
    dir: TempDir,
    release: Release,
    method: install::Method,
}
enum Outcome {
    Checked(Result<Release>),
    Downloaded(Result<Prepared>),
    Installed(Result<()>),
}
pub(super) struct Updates {
    pub cache: Cache,
    pub phase: Phase,
    pub error: Option<String>,
    prepared: Option<Prepared>,
    pub quit_download: CancellationToken,
    receive: Option<Receiver<Outcome>>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start the update worker".into())
}
impl Updates {
    pub fn new(storage: Option<&dyn eframe::Storage>) -> Self {
        let cache: Cache = storage
            .and_then(|s| eframe::get_value(s, "update-cache"))
            .unwrap_or_default();
        let error = cache
            .failed
            .then(|| "The last release check failed. Try again when online.".into());
        Self {
            cache,
            phase: Phase::Idle,
            error,
            prepared: None,
            quit_download: CancellationToken::new(),
            receive: None,
        }
    }
    pub fn available(&self) -> Option<&Release> {
        self.cache.latest.as_ref().filter(|r| r.newer_than(CURRENT))
    }
    pub fn checked(&self) -> bool {
        self.cache.latest.is_some()
    }
    pub fn downloadable(&self) -> bool {
        self.available().is_some_and(|r| r.asset.is_some())
    }
    fn start(
        &mut self,
        ctx: &egui::Context,
        phase: Phase,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.error = None;
        self.phase = phase;
        self.receive = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = work();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    pub fn check(&mut self, ctx: &egui::Context) {
        if self.phase != Phase::Idle {
            return;
        }
        self.cache.attempted = now();
        self.start(ctx, Phase::Checking, || {
            Outcome::Checked((|| runtime()?.block_on(metadata(&client()?)))())
        });
    }
    pub fn prepare(&mut self, ctx: &egui::Context) {
        if self.phase != Phase::Idle {
            return;
        }
        let Some(version) = self.available().map(|r| r.version.clone()) else {
            return;
        };
        let stop = self.quit_download.clone();
        self.start(ctx, Phase::Downloading, move || {
            Outcome::Downloaded(prepare(&version, &stop))
        });
    }
    pub fn install(&mut self, ctx: &egui::Context) {
        if self.phase != Phase::Ready {
            return;
        }
        let Some(package) = self.prepared.take() else {
            return;
        };
        self.start(ctx, Phase::Installing, move || {
            Outcome::Installed(package.method.install(&package.dir, &package.release))
        });
    }
    pub fn ready_with_brew(&self) -> bool {
        self.prepared.as_ref().is_some_and(|p| p.method.is_brew())
    }
    /// Returns true only after an explicitly requested installation succeeded.
    pub fn poll(&mut self, ctx: &egui::Context, automatic: bool) -> bool {
        if let Some(rx) = &self.receive {
            match rx.try_recv() {
                Ok(done) => {
                    self.receive = None;
                    self.phase = Phase::Idle;
                    match done {
                        Outcome::Checked(Ok(r)) => {
                            self.cache.latest = Some(r);
                            self.cache.failed = false;
                        }
                        Outcome::Checked(Err(e)) => {
                            self.cache.failed = true;
                            self.error = Some(e);
                        }
                        Outcome::Downloaded(Ok(p)) => {
                            self.prepared = Some(p);
                            self.phase = Phase::Ready;
                        }
                        Outcome::Downloaded(Err(e)) | Outcome::Installed(Err(e)) => {
                            self.error = Some(e);
                        }
                        Outcome::Installed(Ok(())) => return true,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receive = None;
                    self.phase = Phase::Idle;
                    self.error = Some("The update worker stopped".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if automatic && self.phase == Phase::Idle {
            let elapsed = now().checked_sub(self.cache.attempted).unwrap_or(DAY);
            if elapsed >= DAY {
                self.check(ctx);
            } else {
                ctx.request_repaint_after(Duration::from_secs(DAY - elapsed));
            }
        }
        false
    }
}
fn prepare(version: &str, stop: &CancellationToken) -> Result<Prepared> {
    let rt = runtime()?;
    let client = client()?;
    let release = rt.block_on(async {
        tokio::select! {
            _ = stop.cancelled() => Err("Update download cancelled".into()),
            r = metadata(&client) => r,
        }
    })?;
    if release.version != version || !release.newer_than(CURRENT) {
        return Err("The release changed. Check for updates again.".into());
    }
    let asset = release
        .asset
        .as_ref()
        .ok_or("There is no verified download for this platform")?;
    let method = install::Method::detect()?;
    let dir = method.staging()?;
    if method.is_brew() {
        method.fetch_brew(&release, stop)?;
    } else {
        let archive = dir.path().join("release.archive");
        let mut file = File::create(&archive).map_err(|_| "Could not create the download")?;
        rt.block_on(async {
            tokio::select! {
                _ = stop.cancelled() => Err("Update download cancelled".into()),
                r = download(&client, &release, asset, &mut file) => r,
            }
        })?;
        drop(file);
        method.unpack(&dir, &release)?;
    }
    Ok(Prepared {
        dir,
        release,
        method,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_check_keeps_known_release_and_respects_daily_interval() {
        let ctx = egui::Context::default();
        let mut updates = Updates::new(None);
        updates.cache.latest = Some(Release {
            version: "90.0.0".into(),
            notes: String::new(),
            asset: None,
        });
        updates.cache.attempted = now();
        let (tx, rx) = mpsc::channel();
        tx.send(Outcome::Checked(Err("offline".into()))).unwrap();
        updates.receive = Some(rx);
        updates.phase = Phase::Checking;
        assert!(!updates.poll(&ctx, true));
        assert_eq!(updates.phase, Phase::Idle);
        assert_eq!(updates.available().unwrap().version, "90.0.0");
        assert_eq!(updates.error.as_deref(), Some("offline"));
        assert!(updates.receive.is_none());
        updates.cache.attempted = 0;
        assert!(!updates.poll(&ctx, false));
        assert!(updates.receive.is_none());
    }

    #[test]
    fn a_corrupt_or_incomplete_download_never_becomes_installable() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let rt = runtime().unwrap();
        let c = Client::builder().no_proxy().build().unwrap();
        let check = |body: &'static [u8], size, digest: &str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let mut request = [0; 2048];
                let _ = s.read(&mut request).unwrap();
                write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                s.write_all(body).unwrap();
            });
            let file = tempfile::tempfile().unwrap();
            let asset = Asset {
                name: String::new(),
                size,
                digest: digest.to_owned(),
            };
            let result = rt.block_on(async {
                let mut file = file;
                save_verified(c.get(url).send().await.unwrap(), &asset, &mut file).await
            });
            server.join().unwrap();
            result
        };
        let digest = format!("{:x}", Sha256::digest(b"good"));
        assert!(check(b"good", 4, &digest).is_ok());
        assert!(check(b"evil", 4, &digest).is_err());
        assert!(check(b"go", 4, &digest).is_err());
        assert!(check(b"oversize", 4, &digest).is_err());
    }

    #[test]
    fn checks_stable_numeric_versions_and_requires_verified_asset() {
        let mut data =
            serde_json::json!({"tag_name":"v0.10.0","draft":false,"prerelease":false,"assets":[]});
        let parse = |v: &serde_json::Value| Release::parse(&serde_json::to_vec(v).unwrap());
        assert!(parse(&data).unwrap().newer_than("0.9.9"));
        assert!(!parse(&data).unwrap().newer_than("0.10.0"));
        assert!(parse(&data).unwrap().asset.is_none());
        data["prerelease"] = true.into();
        assert!(parse(&data).is_err());
        data["prerelease"] = false.into();
        data["tag_name"] = "v1.0.0-rc.1".into();
        assert!(parse(&data).is_err());
    }
}
