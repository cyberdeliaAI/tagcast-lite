use super::{Release, Result, stable};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::TempDir;

#[derive(Debug, PartialEq)]
pub(super) struct Method {
    target: PathBuf,
    #[cfg(target_os = "macos")]
    brew: Option<PathBuf>,
}
#[cfg(target_os = "macos")]
fn ok(command: &mut Command, message: &str) -> Result<()> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.is_ok_and(|s| s.success()) {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn output(command: &mut Command) -> Result<String> {
    let r = command
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "Could not start an update tool")?;
    if !r.status.success() {
        return Err("Could not inspect the app or package manager".into());
    }
    String::from_utf8(r.stdout).map_err(|_| "Unreadable version information".into())
}
impl Method {
    pub fn detect() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|_| "Could not locate this installation")?;
        #[cfg(target_os = "macos")]
        {
            let app=exe.ancestors().nth(3).filter(|p|p.extension().is_some_and(|e|e=="app") &&
                p.join("Contents/MacOS/tagcast-player")==exe)
                .ok_or("In-app installation requires a Tagcast Lite app bundle. Source builds are updated with Git.")?;
            if app
                .components()
                .any(|c| c.as_os_str() == "AppTranslocation")
            {
                return Err(
                    "Move Tagcast Lite to Applications and reopen it before updating.".into(),
                );
            }
            let target = app
                .canonicalize()
                .map_err(|_| "Could not locate the app bundle")?;
            let brew = homebrew(&target);
            Ok(Self { target, brew })
        }
        #[cfg(not(target_os = "macos"))]
        {
            if exe
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "debug" || n == "release")
                && exe
                    .ancestors()
                    .any(|p| p.file_name().is_some_and(|n| n == "target"))
            {
                return Err("This is a Cargo build. Update the source checkout with Git.".into());
            }
            Ok(Self { target: exe })
        }
    }
    pub fn is_brew(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.brew.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
    pub fn staging(&self) -> Result<TempDir> {
        if self.is_brew() {
            return tempfile::tempdir().map_err(|_| "Could not create the download folder".into());
        }
        let parent = self.target.parent().ok_or("Invalid installation folder")?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".tagcast-update-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        builder.tempdir_in(parent)
            .map_err(|_|"This installation folder is read-only. Move the app to a writable folder or use your package manager.".into())
    }
    fn installed_version(&self) -> Result<String> {
        package_version(&self.target)
    }
    pub fn unpack(&self, dir: &TempDir, release: &Release) -> Result<()> {
        let archive = dir.path().join("release.archive");
        #[cfg(target_os = "macos")]
        {
            ok(
                Command::new("/usr/bin/ditto")
                    .args(["-x", "-k"])
                    .arg(archive)
                    .arg(dir.path()),
                "Could not unpack the Mac app",
            )?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Copy only the named executable. Archive paths never become destination paths.
            let file =
                fs::File::create(candidate(dir)).map_err(|_| "Could not stage the player")?;
            #[cfg(windows)]
            let (tar, member) = (
                PathBuf::from(
                    std::env::var_os("SystemRoot").ok_or("Windows system folder unavailable")?,
                )
                .join("System32/tar.exe"),
                "tagcast-player.exe",
            );
            #[cfg(not(windows))]
            let (tar, member) = (PathBuf::from("/usr/bin/tar"), "./tagcast-player");
            let status = Command::new(tar)
                .arg("-xOf")
                .arg(archive)
                .arg(member)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(file)
                .status();
            if !status.is_ok_and(|s| s.success()) {
                return Err("Could not extract the player from its release archive".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(candidate(dir), fs::Permissions::from_mode(0o755))
                    .map_err(|_| "Could not prepare the player executable")?;
            }
        }
        verify(&candidate(dir), release)
    }
    pub fn install(&self, dir: &TempDir, release: &Release) -> Result<()> {
        let data = tagcast_player::account::data_dir().ok_or("Could not locate the update lock")?;
        fs::create_dir_all(&data).map_err(|_| "Could not create the update lock folder")?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data.join("update.lock"))
            .map_err(|_| "Could not open the update lock")?;
        lock.try_lock()
            .map_err(|_| "Another copy of Tagcast Lite is installing an update")?;
        if &Self::detect()? != self || !release.newer_than(&self.installed_version()?) {
            return Err("The installation changed. Reopen the player and check again.".into());
        }
        #[cfg(target_os = "macos")]
        if let Some(brew) = &self.brew {
            verify_cask(brew, release)?;
            ok(
                brew_cmd(brew).args(["upgrade", "--cask", CASK]),
                "Homebrew could not upgrade Tagcast Lite. Check your Homebrew installation and try again.",
            )?;
            return verify(&self.target, release);
        }
        let staged = candidate(dir);
        verify(&staged, release)?;
        replace(&staged, &self.target)
    }
    #[cfg(target_os = "macos")]
    pub fn fetch_brew(
        &self,
        release: &Release,
        stop: &tagcast_player::CancellationToken,
    ) -> Result<()> {
        let brew = self
            .brew
            .as_ref()
            .ok_or("Homebrew installation unavailable")?;
        ok(
            brew_cmd(brew).args(["update", "--quiet"]),
            "Homebrew could not refresh its taps",
        )?;
        if stop.is_cancelled() {
            return Err("Update download cancelled".into());
        }
        verify_cask(brew, release)?;
        ok(
            brew_cmd(brew).args(["fetch", "--cask", CASK]),
            "Homebrew could not download the verified cask",
        )
    }
    #[cfg(not(target_os = "macos"))]
    pub fn fetch_brew(&self, _: &Release, _: &tagcast_player::CancellationToken) -> Result<()> {
        Err("Homebrew is unavailable".into())
    }
}
fn candidate(dir: &TempDir) -> PathBuf {
    dir.path().join(if cfg!(target_os = "macos") {
        "Tagcast Lite.app"
    } else if cfg!(windows) {
        "tagcast-player.exe"
    } else {
        "tagcast-player"
    })
}
fn package_version(path: &Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    let version = output(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(path.join("Contents/Info.plist")),
    )?;
    #[cfg(not(target_os = "macos"))]
    let version = output(Command::new(path).arg("--version"))?
        .trim()
        .strip_prefix("tagcast-player ")
        .ok_or("Unexpected player executable")?
        .to_owned();
    let version = version.trim();
    stable(version)
        .map(|v| v.to_string())
        .ok_or_else(|| "Invalid application version".into())
}
fn verify(path: &Path, release: &Release) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Match the Developer ID and bundle identifier of the notarized Lite distribution.
        let requirement = "=anchor apple generic and identifier \"nl.cyberdelia.tagcast-player\" and certificate leaf[subject.OU] = \"H3G46VS6J7\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists";
        ok(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict", "-R", requirement])
                .arg(path),
            "The update does not have Tagcast Lite’s Developer ID signature",
        )?;
        ok(
            Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(path),
            "macOS could not confirm the notarized update",
        )?;
    }
    if package_version(path)? != release.version {
        return Err("The package version does not match the selected release".into());
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn replace(staged: &Path, target: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path =
        |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|_| "Invalid installation path");
    let from = path(staged)?;
    let to = path(target)?;
    // Both paths are on the same filesystem and the C strings live through the syscall.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(
            "macOS could not replace the app. The previous version is still in place.".into(),
        );
    }
    Ok(())
}
#[cfg(all(unix, not(target_os = "macos")))]
fn replace(staged: &Path, target: &Path) -> Result<()> {
    fs::rename(staged, target)
        .map_err(|_| "Could not replace the player in its installation folder".into())
}
#[cfg(windows)]
fn replace(staged: &Path, _: &Path) -> Result<()> {
    self_replace::self_replace(staged).map_err(|_| {
        "Windows could not replace the player. Close other copies and try again.".into()
    })
}

#[cfg(target_os = "macos")]
const CASK: &str = "cyberdeliaAI/apps/tagcast-lite";
#[cfg(target_os = "macos")]
fn homebrew(target: &Path) -> Option<PathBuf> {
    let prefixes = [
        std::env::var_os("HOMEBREW_PREFIX").map(PathBuf::from),
        Some("/opt/homebrew".into()),
        Some("/usr/local".into()),
    ];
    for prefix in prefixes.into_iter().flatten().filter(|p| p.is_absolute()) {
        let room = prefix.join("Caskroom/tagcast-lite");
        if let Ok(versions) = fs::read_dir(room)
            && versions.flatten().any(|v| {
                v.path()
                    .join("Tagcast Lite.app")
                    .canonicalize()
                    .ok()
                    .as_deref()
                    == Some(target)
            })
        {
            return Some(prefix.join("bin/brew"));
        }
    }
    None
}
#[cfg(target_os = "macos")]
fn brew_cmd(brew: &Path) -> Command {
    let mut c = Command::new(brew);
    c.env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_INSTALL_CLEANUP", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("NONINTERACTIVE", "1");
    c
}
#[cfg(target_os = "macos")]
fn verify_cask(brew: &Path, release: &Release) -> Result<()> {
    let asset = release.asset.as_ref().ok_or("Missing platform download")?;
    let data = output(brew_cmd(brew).args(["info", "--json=v2", "--cask", CASK]))?;
    let j: serde_json::Value =
        serde_json::from_str(&data).map_err(|_| "Unreadable cask information")?;
    let c = &j["casks"][0];
    let url = format!(
        "{}/releases/download/v{}/{}",
        super::REPOSITORY,
        release.version,
        asset.name
    );
    if c["version"].as_str() != Some(&release.version)
        || c["url"].as_str() != Some(&url)
        || c["sha256"].as_str() != Some(&asset.digest)
    {
        return Err(
            "The Homebrew tap does not yet offer this verified release. Try again later.".into(),
        );
    }
    Ok(())
}
