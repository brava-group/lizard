//! Checked macOS updates from published GitHub releases. No update is installed without a click.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lizard_ui_egui::UpdateStatus;
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const HELPER_ARG: &str = "--lizard-install-update";
const RELEASES_API: &str = "https://api.github.com/repos/brava-group/lizard/releases/latest";
const TEAM: &str = "2L268W62AW";
const BUNDLE_ID: &str = "com.brava.lizard";
const MAX_DMG_BYTES: u64 = 600 * 1024 * 1024;
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Debug)]
struct Release {
    version: Version,
    url: String,
    digest: String,
}

#[derive(Debug)]
struct Prepared {
    version: Version,
    app: PathBuf,
    helper: PathBuf,
    work: PathBuf,
}

enum Reply {
    Checked(Result<Option<Release>, String>),
    Prepared(Result<Prepared, String>),
}

pub struct Updater {
    installed: Option<PathBuf>,
    status: UpdateStatus,
    pending: Option<Receiver<Reply>>,
    release: Option<Release>,
    prepared: Option<Prepared>,
    last_check: Option<Instant>,
}

impl Updater {
    pub fn new() -> Self {
        let installed = installed_bundle();
        let mut this = Self { installed, status: UpdateStatus::Unavailable, pending: None, release: None, prepared: None, last_check: None };
        if this.installed.is_some() {
            this.check();
        }
        this
    }

    pub fn status(&mut self) -> UpdateStatus {
        if matches!(&self.status, UpdateStatus::Current | UpdateStatus::Error(_))
            && self.last_check.is_some_and(|last| last.elapsed() >= CHECK_INTERVAL)
        {
            self.check();
        }
        let received = self.pending.as_ref().map(Receiver::try_recv);
        if matches!(&received, Some(Err(TryRecvError::Disconnected))) {
            self.pending = None;
            self.status = UpdateStatus::Error("The update check stopped unexpectedly".into());
        }
        if let Some(Ok(reply)) = received {
            self.pending = None;
            match reply {
                Reply::Checked(Ok(Some(release))) => {
                    self.status = UpdateStatus::Available(release.version.to_string());
                    self.release = Some(release);
                }
                Reply::Checked(Ok(None)) => self.status = UpdateStatus::Current,
                Reply::Checked(Err(error)) | Reply::Prepared(Err(error)) => self.status = UpdateStatus::Error(error),
                Reply::Prepared(Ok(prepared)) => {
                    self.status = UpdateStatus::Ready(prepared.version.to_string());
                    self.prepared = Some(prepared);
                }
            }
        }
        self.status.clone()
    }

    pub fn check(&mut self) {
        if self.installed.is_none() || self.pending.is_some() || self.prepared.is_some() {
            return;
        }
        self.release = None;
        self.status = UpdateStatus::Checking;
        self.last_check = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Reply::Checked(fetch_release()));
        });
    }

    pub fn prepare(&mut self) {
        let (Some(installed), Some(release)) = (self.installed.clone(), self.release.clone()) else { return };
        if self.pending.is_some() || self.prepared.is_some() {
            return;
        }
        self.status = UpdateStatus::Preparing(release.version.to_string());
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Reply::Prepared(prepare(&installed, &release)));
        });
    }

    pub fn install(&mut self) -> Result<(), String> {
        let Some(prepared) = self.prepared.as_ref() else { return Err("No verified update is ready".into()) };
        let installed = self.installed.as_ref().ok_or("Lizard is not running from an app bundle")?;
        Command::new(&prepared.helper)
            .args([HELPER_ARG, &std::process::id().to_string()])
            .arg(installed)
            .arg(&prepared.app)
            .arg(&prepared.work)
            .spawn()
            .map_err(|error| format!("Could not start the update installer: {error}"))?;
        self.status = UpdateStatus::Installing;
        Ok(())
    }

    pub fn cleanup(&mut self) {
        if self.status == UpdateStatus::Installing {
            return;
        }
        if let Some(prepared) = self.prepared.take() {
            let _ = fs::remove_dir_all(prepared.app);
            let _ = fs::remove_dir_all(prepared.work);
        }
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn installed_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    if executable.file_name()? != "Lizard" || executable.parent()?.file_name()? != "MacOS" {
        return None;
    }
    let bundle = executable.parent()?.parent()?.parent()?.to_path_buf();
    if bundle.extension()? != "app" || !bundle.is_dir() {
        return None;
    }
    Some(bundle)
}

fn fetch_release() -> Result<Option<Release>, String> {
    let output = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "User-Agent: Lizard-Updater",
            RELEASES_API,
        ])
        .output()
        .map_err(|error| format!("Could not check for updates: {error}"))?;
    if !output.status.success() {
        return Err("The Lizard update channel is not available yet".into());
    }
    if output.stdout.len() > 1024 * 1024 {
        return Err("The update response is too large".into());
    }
    let body: Value = serde_json::from_slice(&output.stdout).map_err(|error| format!("Invalid update response: {error}"))?;
    parse_release(&body, env!("CARGO_PKG_VERSION"), std::env::consts::ARCH)
}

fn parse_release(body: &Value, current: &str, arch: &str) -> Result<Option<Release>, String> {
    if body["draft"].as_bool() != Some(false) || body["prerelease"].as_bool() != Some(false) {
        return Err("The update feed did not return a published stable release".into());
    }
    let tag = body["tag_name"].as_str().ok_or("The update has no version tag")?;
    let version = Version::parse(tag.strip_prefix('v').ok_or("Invalid update tag")?).map_err(|error| format!("Invalid update version: {error}"))?;
    let current = Version::parse(current).map_err(|error| format!("Invalid installed version: {error}"))?;
    if version <= current {
        return Ok(None);
    }
    let name = format!("lizard-{version}-macos-{arch}.dmg");
    let expected = format!("https://github.com/brava-group/lizard/releases/download/{tag}/{name}");
    let asset = body["assets"].as_array().and_then(|assets| assets.iter().find(|asset| asset["name"].as_str() == Some(&name)));
    let asset = asset.ok_or("The new version has no installer for this Mac")?;
    let url = asset["browser_download_url"].as_str().ok_or("The update has no download URL")?;
    if url != expected {
        return Err("The update points outside the Lizard release channel".into());
    }
    let digest = asset["digest"].as_str().and_then(|value| value.strip_prefix("sha256:")).ok_or("The update has no SHA-256 digest")?;
    if digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || asset["size"].as_u64().is_none_or(|size| size == 0 || size > MAX_DMG_BYTES)
    {
        return Err("The update has invalid size or digest".into());
    }
    Ok(Some(Release { version, url: url.into(), digest: digest.to_ascii_lowercase() }))
}

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program).args(args).output().map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
}

fn verify_bundle(app: &Path, version: &Version) -> Result<(), String> {
    if !fs::symlink_metadata(app).map_err(|error| error.to_string())?.file_type().is_dir() {
        return Err("The downloaded app is not a directory".into());
    }
    let app = app.to_str().ok_or("Invalid app path")?;
    command("/usr/bin/codesign", &["--verify", "--strict", "--deep", app])?;
    let signature = command("/usr/bin/codesign", &["--display", "--verbose=4", app])?;
    if !signature.lines().any(|line| line == format!("Identifier={BUNDLE_ID}"))
        || !signature.lines().any(|line| line == format!("TeamIdentifier={TEAM}"))
    {
        return Err("The update is not signed for Lizard by its Developer ID team".into());
    }
    command("/usr/sbin/spctl", &["--assess", "--type", "execute", app])?;
    let plist = Path::new(app).join("Contents/Info.plist");
    let plist = plist.to_str().ok_or("Invalid app metadata path")?;
    let id = command("/usr/bin/plutil", &["-extract", "CFBundleIdentifier", "raw", "-o", "-", plist])?;
    let found = command("/usr/bin/plutil", &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", plist])?;
    if id.trim() != BUNDLE_ID || found.trim() != version.to_string() {
        return Err("The downloaded app has a different identity or version".into());
    }
    Ok(())
}

fn prepare(installed: &Path, release: &Release) -> Result<Prepared, String> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
    let work = std::env::temp_dir().join(format!("lizard-update-{}-{stamp}", std::process::id()));
    let parent = installed.parent().ok_or("Invalid installed app path")?;
    let stage = parent.join(format!(".Lizard-update-{stamp}.app"));
    fs::create_dir(&work).map_err(|error| format!("Could not prepare update folder: {error}"))?;
    let result = (|| {
        let dmg = work.join("update.dmg");
        let status = Command::new("/usr/bin/curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "10",
                "--max-time",
                "600",
                "--max-filesize",
                "629145600",
                "--output",
            ])
            .arg(&dmg)
            .arg(&release.url)
            .status()
            .map_err(|error| format!("Could not download update: {error}"))?;
        if !status.success() {
            return Err("The update download failed".into());
        }
        let mut file = File::open(&dmg).map_err(|error| error.to_string())?;
        if file.metadata().map_err(|error| error.to_string())?.len() > MAX_DMG_BYTES {
            return Err("The update is too large".into());
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        if format!("{:x}", hasher.finalize()) != release.digest {
            return Err("The update checksum does not match GitHub's release".into());
        }
        command(
            "/usr/sbin/spctl",
            &["--assess", "--type", "open", "--context", "context:primary-signature", dmg.to_str().ok_or("Invalid download path")?],
        )?;
        let mount = work.join("mount");
        fs::create_dir(&mount).map_err(|error| error.to_string())?;
        command(
            "/usr/bin/hdiutil",
            &[
                "attach",
                "-readonly",
                "-nobrowse",
                "-mountpoint",
                mount.to_str().ok_or("Invalid mount path")?,
                dmg.to_str().ok_or("Invalid download path")?,
            ],
        )?;
        let source = mount.join("Lizard.app");
        let copied = Command::new("/usr/bin/ditto").arg(&source).arg(&stage).status().map_err(|error| format!("Could not copy the update: {error}"));
        let detached = command("/usr/bin/hdiutil", &["detach", mount.to_str().ok_or("Invalid mount path")?]);
        copied?.success().then_some(()).ok_or("Could not stage the update beside Lizard.app")?;
        detached?;
        verify_bundle(&stage, &release.version)?;
        let helper = work.join("lizard-update-helper");
        fs::copy(std::env::current_exe().map_err(|error| error.to_string())?, &helper).map_err(|error| error.to_string())?;
        Ok(Prepared { version: release.version.clone(), app: stage.clone(), helper, work: work.clone() })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Run from a temporary copy of the signed executable after the UI has asked to quit.
pub fn run_helper(args: &[String]) -> i32 {
    let result = (|| -> Result<(), String> {
        let pid = args.get(1).ok_or("Missing parent process")?.parse::<u32>().map_err(|error| error.to_string())?;
        let installed = PathBuf::from(args.get(2).ok_or("Missing installed app")?);
        let stage = PathBuf::from(args.get(3).ok_or("Missing staged app")?);
        let work = PathBuf::from(args.get(4).ok_or("Missing update folder")?);
        if installed.extension().and_then(|extension| extension.to_str()) != Some("app")
            || stage.parent() != installed.parent()
            || !stage.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with(".Lizard-update-") && name.ends_with(".app"))
        {
            return Err("Invalid update destination".into());
        }
        let installed_plist = installed.join("Contents/Info.plist");
        let installed_plist = installed_plist.to_str().ok_or("Invalid installed app metadata")?;
        let installed_id = command("/usr/bin/plutil", &["-extract", "CFBundleIdentifier", "raw", "-o", "-", installed_plist])?;
        if installed_id.trim() != BUNDLE_ID {
            return Err("The installed app is not Lizard".into());
        }
        let installed_version = command("/usr/bin/plutil", &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", installed_plist])?;
        let installed_version = Version::parse(installed_version.trim()).map_err(|error| error.to_string())?;
        for _ in 0..600 {
            if !Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "pid="])
                .output()
                .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
        {
            return Err("Lizard did not quit; the update was not installed".into());
        }
        let version = stage.join("Contents/Info.plist");
        let version = command(
            "/usr/bin/plutil",
            &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", version.to_str().ok_or("Invalid app metadata")?],
        )?;
        let version = Version::parse(version.trim()).map_err(|error| error.to_string())?;
        if version <= installed_version {
            return Err("The staged app is not newer than the installed app".into());
        }
        verify_bundle(&stage, &version)?;
        let backup = installed.with_file_name(format!(".Lizard-before-update-{pid}.app"));
        fs::rename(&installed, &backup).map_err(|error| format!("Could not move the installed app: {error}"))?;
        if let Err(error) = fs::rename(&stage, &installed) {
            let _ = fs::rename(&backup, &installed);
            return Err(format!("Could not install the update: {error}"));
        }
        if let Err(error) = command("/usr/bin/open", &["-a", installed.to_str().ok_or("Invalid installed path")?]) {
            let _ = fs::rename(&installed, &stage);
            let _ = fs::rename(&backup, &installed);
            return Err(error);
        }
        let _ = fs::remove_dir_all(backup);
        let _ = fs::remove_dir_all(work);
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Lizard update failed: {error}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(version: &str, digest: &str) -> Value {
        let name = format!("lizard-{version}-macos-aarch64.dmg");
        json!({"draft":false,"prerelease":false,"tag_name":format!("v{version}"),"assets":[{"name":name,"browser_download_url":format!("https://github.com/brava-group/lizard/releases/download/v{version}/lizard-{version}-macos-aarch64.dmg"),"digest":format!("sha256:{digest}"),"size":100}]})
    }

    #[test]
    fn only_new_published_signed_channel_releases_are_offered() {
        let digest = "a".repeat(64);
        let good = release("0.9.2", &digest);
        assert!(parse_release(&good, "0.9.1", "aarch64").is_ok_and(|found| found.is_some()));
        assert!(parse_release(&good, "0.9.2", "aarch64").is_ok_and(|found| found.is_none()));
        let mut wrong = good.clone();
        wrong["assets"][0]["browser_download_url"] = json!("https://example.com/update.dmg");
        assert!(parse_release(&wrong, "0.9.1", "aarch64").is_err());
        let mut draft = good;
        draft["draft"] = json!(true);
        assert!(parse_release(&draft, "0.9.1", "aarch64").is_err());
    }
}
