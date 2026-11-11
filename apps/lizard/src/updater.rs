//! Checked macOS updates from published GitHub releases.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lizard_ui_egui::{UpdateProgress, UpdateStatus, UpdateStep};
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const HELPER_ARG: &str = "--lizard-install-update";
const RELEASES_API: &str = "https://api.github.com/repos/brava-group/lizard/releases/latest";
const TEAM: &str = "2L268W62AW";
const BUNDLE_ID: &str = "com.brava.lizard";
const MAX_DMG_BYTES: u64 = 600 * 1024 * 1024;
const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug)]
struct Release {
    version: Version,
    url: String,
    digest: String,
    size: u64,
}

#[derive(Debug)]
struct Prepared {
    version: Version,
    app: PathBuf,
    destination: PathBuf,
    work: PathBuf,
}

enum Reply {
    Checked(Result<Option<Release>, String>),
    Progress(UpdateProgress),
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
        loop {
            let received = self.pending.as_ref().map(Receiver::try_recv);
            match received {
                Some(Ok(Reply::Progress(progress))) => {
                    if let UpdateStatus::Preparing(version, _) = &self.status {
                        self.status = UpdateStatus::Preparing(version.clone(), progress);
                    }
                }
                Some(Ok(reply)) => {
                    self.pending = None;
                    match reply {
                        Reply::Checked(Ok(Some(release))) => {
                            self.status = UpdateStatus::Available(release.version.to_string());
                            self.release = Some(release);
                        }
                        Reply::Checked(Ok(None)) => self.status = UpdateStatus::Current,
                        Reply::Checked(Err(error)) => self.status = UpdateStatus::Error(error),
                        Reply::Prepared(Err(error)) => {
                            let version = self.release.as_ref().map(|release| release.version.to_string()).unwrap_or_default();
                            self.status = UpdateStatus::Failed(version, error);
                        }
                        Reply::Prepared(Ok(prepared)) => {
                            self.status = UpdateStatus::Ready(prepared.version.to_string());
                            self.prepared = Some(prepared);
                        }
                        Reply::Progress(_) => {}
                    }
                    break;
                }
                Some(Err(TryRecvError::Disconnected)) => {
                    self.pending = None;
                    let error = "The update process stopped unexpectedly".to_owned();
                    self.status = match &self.status {
                        UpdateStatus::Preparing(version, _) => UpdateStatus::Failed(version.clone(), error),
                        _ => UpdateStatus::Error(error),
                    };
                    break;
                }
                Some(Err(TryRecvError::Empty)) | None => break,
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
        if self.pending.is_some() {
            return;
        }
        if self.prepared.is_some() {
            self.status = UpdateStatus::Ready(release.version.to_string());
            return;
        }
        self.status = UpdateStatus::Preparing(release.version.to_string(), UpdateProgress::default());
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Reply::Prepared(prepare(&installed, &release, &tx)));
        });
    }

    pub fn install(&mut self) -> Result<(), String> {
        let Some(prepared) = self.prepared.as_ref() else { return Err("No verified update is ready".into()) };
        let installed = self.installed.as_ref().ok_or("Lizard is not running from an app bundle")?;
        let result = (|| -> Result<(), String> {
            // The signed executable must run from its app bundle. Copying it into `work`
            // invalidates the bundle's sealed resources and macOS kills it on launch.
            let log = File::create(prepared.work.join("install.log")).map_err(|error| error.to_string())?;
            Command::new(installed.join("Contents/MacOS/Lizard"))
                .args([HELPER_ARG, &std::process::id().to_string()])
                .arg(installed)
                .arg(&prepared.destination)
                .arg(&prepared.app)
                .arg(&prepared.work)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(log))
                .spawn()
                .map_err(|error| format!("Could not start the update installer: {error}"))?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.status = UpdateStatus::Installing;
                Ok(())
            }
            Err(error) => {
                self.status = UpdateStatus::Failed(prepared.version.to_string(), error.clone());
                Err(error)
            }
        }
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
    let assets = body["assets"].as_array().ok_or("The update has no installers")?;
    let (name, asset) = [format!("lizard-{version}-macos-{arch}.dmg"), format!("lizard-{version}-macos-universal.dmg")]
        .into_iter()
        .find_map(|name| assets.iter().find(|asset| asset["name"].as_str() == Some(name.as_str())).map(|asset| (name, asset)))
        .ok_or("The new version has no installer for this Mac")?;
    let expected = format!("https://github.com/brava-group/lizard/releases/download/{tag}/{name}");
    let url = asset["browser_download_url"].as_str().ok_or("The update has no download URL")?;
    if url != expected {
        return Err("The update points outside the Lizard release channel".into());
    }
    let digest = asset["digest"].as_str().and_then(|value| value.strip_prefix("sha256:")).ok_or("The update has no SHA-256 digest")?;
    let size = asset["size"].as_u64().ok_or("The update has no size")?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) || size == 0 || size > MAX_DMG_BYTES {
        return Err("The update has invalid size or digest".into());
    }
    Ok(Some(Release { version, url: url.into(), digest: digest.to_ascii_lowercase(), size }))
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

fn verify_destination(destination: &Path, update_version: &Version) -> Result<(), String> {
    if !destination.exists() {
        return Ok(());
    }
    let plist = destination.join("Contents/Info.plist");
    let current = command(
        "/usr/bin/plutil",
        &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", plist.to_str().ok_or("Invalid existing app metadata")?],
    )?;
    let current = Version::parse(current.trim()).map_err(|error| error.to_string())?;
    verify_bundle(destination, &current)?;
    if update_version <= &current {
        return Err("This version or a newer one is already installed in Applications".into());
    }
    Ok(())
}

fn update_destination(installed: &Path, stamp: u128) -> Result<(PathBuf, PathBuf), String> {
    let mut candidates = vec![installed.to_path_buf(), PathBuf::from("/Applications/Lizard.app")];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/Lizard.app"));
    }
    writable_destination(candidates, stamp)
}

fn writable_destination(candidates: impl IntoIterator<Item = PathBuf>, stamp: u128) -> Result<(PathBuf, PathBuf), String> {
    for destination in candidates {
        let Some(parent) = destination.parent() else { continue };
        if fs::create_dir_all(parent).is_err() {
            continue;
        }
        let stage = parent.join(format!(".Lizard-update-{stamp}.app"));
        if fs::create_dir(&stage).is_ok() {
            return Ok((destination, stage));
        }
    }
    Err("Could not write an update to Applications. Check that your Mac has free space and your account can install apps.".into())
}

fn percentage(done: u64, total: u64, start: u8, span: u8) -> u8 {
    start + ((done.min(total) * u64::from(span)) / total.max(1)) as u8
}

fn report(tx: &mpsc::Sender<Reply>, step: UpdateStep, percent: u8) {
    let _ = tx.send(Reply::Progress(UpdateProgress { step, percent }));
}

fn download(release: &Release, dmg: &Path, tx: &mpsc::Sender<Reply>) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/curl")
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
        .arg(dmg)
        .arg(&release.url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not download update: {error}"))?;
    let mut last = 0;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| format!("Could not download update: {error}"))? {
            return if status.success() { Ok(()) } else { Err(format!("The update download failed ({status})")) };
        }
        let received = fs::metadata(dmg).map_or(0, |metadata| metadata.len());
        let percent = percentage(received, release.size, 0, 70);
        if percent > last {
            report(tx, UpdateStep::Downloading, percent);
            last = percent;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn prepare(installed: &Path, release: &Release, tx: &mpsc::Sender<Reply>) -> Result<Prepared, String> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
    let work = std::env::temp_dir().join(format!("lizard-update-{}-{stamp}", std::process::id()));
    let (destination, stage) = update_destination(installed, stamp)?;
    if let Err(error) = fs::create_dir(&work) {
        let _ = fs::remove_dir(&stage);
        return Err(format!("Could not prepare update folder: {error}"));
    }
    let result = (|| {
        let dmg = work.join("update.dmg");
        download(release, &dmg, tx)?;
        report(tx, UpdateStep::Verifying, 70);
        let mut file = File::open(&dmg).map_err(|error| error.to_string())?;
        if file.metadata().map_err(|error| error.to_string())?.len() != release.size {
            return Err("The update size does not match GitHub's release".into());
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut checked = 0_u64;
        let mut last = 70;
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            checked += count as u64;
            let percent = percentage(checked, release.size, 70, 15);
            if percent > last {
                report(tx, UpdateStep::Verifying, percent);
                last = percent;
            }
        }
        if format!("{:x}", hasher.finalize()) != release.digest {
            return Err("The update checksum does not match GitHub's release".into());
        }
        report(tx, UpdateStep::Verifying, 86);
        command(
            "/usr/sbin/spctl",
            &["--assess", "--type", "open", "--context", "context:primary-signature", dmg.to_str().ok_or("Invalid download path")?],
        )?;
        report(tx, UpdateStep::Staging, 90);
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
        report(tx, UpdateStep::Staging, 93);
        let source = mount.join("Lizard.app");
        let copied = Command::new("/usr/bin/ditto").arg(&source).arg(&stage).status().map_err(|error| format!("Could not copy the update: {error}"));
        let detached = command("/usr/bin/hdiutil", &["detach", mount.to_str().ok_or("Invalid mount path")?]);
        copied?.success().then_some(()).ok_or("Could not stage the update in Applications")?;
        detached?;
        report(tx, UpdateStep::Staging, 96);
        verify_bundle(&stage, &release.version)?;
        report(tx, UpdateStep::Staging, 98);
        verify_destination(&destination, &release.version)?;
        report(tx, UpdateStep::Staging, 99);
        Ok(Prepared { version: release.version.clone(), app: stage.clone(), destination, work: work.clone() })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Continue running inside the signed bundle after the UI has asked to quit.
pub fn run_helper(args: &[String]) -> i32 {
    let result = (|| -> Result<(), String> {
        let pid = args.get(1).ok_or("Missing parent process")?.parse::<u32>().map_err(|error| error.to_string())?;
        let installed = PathBuf::from(args.get(2).ok_or("Missing running app")?);
        let destination = PathBuf::from(args.get(3).ok_or("Missing install destination")?);
        let stage = PathBuf::from(args.get(4).ok_or("Missing staged app")?);
        let work = PathBuf::from(args.get(5).ok_or("Missing update folder")?);
        if installed.extension().and_then(|extension| extension.to_str()) != Some("app")
            || destination.file_name().and_then(|name| name.to_str()) != Some("Lizard.app")
            || stage.parent() != destination.parent()
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
        verify_destination(&destination, &version)?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
        let backup = destination.with_file_name(format!(".Lizard-before-update-{pid}-{stamp}.app"));
        let had_previous = destination.exists();
        if had_previous {
            fs::rename(&destination, &backup).map_err(|error| format!("Could not move the installed app: {error}"))?;
        }
        if let Err(error) = fs::rename(&stage, &destination) {
            if had_previous {
                let _ = fs::rename(&backup, &destination);
            }
            return Err(format!("Could not install the update: {error}"));
        }
        if let Err(error) = command("/usr/bin/open", &["-n", "-a", destination.to_str().ok_or("Invalid installed path")?]) {
            let _ = fs::rename(&destination, &stage);
            if had_previous {
                let _ = fs::rename(&backup, &destination);
            }
            return Err(error);
        }
        if had_previous {
            let _ = fs::remove_dir_all(backup);
        }
        let _ = fs::remove_dir_all(work);
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Lizard update failed: {error}");
        let _ = rfd::MessageDialog::new().set_title("Lizard update failed").set_description(&error).set_level(rfd::MessageLevel::Error).show();
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

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

    #[test]
    fn universal_dmg_updates_both_mac_architectures() {
        let digest = "a".repeat(64);
        let mut universal = release("0.9.2", &digest);
        let name = "lizard-0.9.2-macos-universal.dmg";
        universal["assets"][0]["name"] = json!(name);
        universal["assets"][0]["browser_download_url"] = json!(format!("https://github.com/brava-group/lizard/releases/download/v0.9.2/{name}"));
        for arch in ["aarch64", "x86_64"] {
            assert!(parse_release(&universal, "0.9.1", arch).is_ok_and(|found| found.is_some()));
        }
    }

    #[test]
    fn update_stages_beside_writable_fallback_when_running_volume_cannot_be_written() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = std::env::temp_dir().join(format!("lizard-updater-test-{}", std::process::id()));
        fs::create_dir(&temporary)?;
        let destination = temporary.join("Lizard.app");
        let (found, stage) = writable_destination([PathBuf::from("/dev/null/Lizard.app"), destination.clone()], 123)?;
        assert_eq!(found, destination);
        assert_eq!(stage.parent(), destination.parent());
        assert!(stage.is_dir());
        fs::remove_dir_all(temporary)?;
        Ok(())
    }

    #[test]
    fn download_and_verification_percentages_are_bounded() {
        assert_eq!(percentage(0, 200, 0, 70), 0);
        assert_eq!(percentage(100, 200, 0, 70), 35);
        assert_eq!(percentage(200, 200, 0, 70), 70);
        assert_eq!(percentage(400, 200, 70, 15), 85);
    }

    #[test]
    fn progress_does_not_finish_the_download() -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = mpsc::channel();
        let mut updater = Updater {
            installed: None,
            status: UpdateStatus::Preparing("0.9.7".into(), UpdateProgress::default()),
            pending: Some(rx),
            release: None,
            prepared: None,
            last_check: None,
        };
        assert!(tx.send(Reply::Progress(UpdateProgress { step: UpdateStep::Downloading, percent: 35 })).is_ok());
        assert_eq!(updater.status(), UpdateStatus::Preparing("0.9.7".into(), UpdateProgress { step: UpdateStep::Downloading, percent: 35 }));
        assert!(updater.pending.is_some());
        Ok(())
    }

    #[test]
    fn installer_launches_from_the_app_bundle() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "lizard-updater-helper-test-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let installed = root.join("Lizard.app");
        let executable = installed.join("Contents/MacOS/Lizard");
        let work = root.join("work");
        fs::create_dir_all(executable.parent().ok_or("missing executable parent")?)?;
        fs::create_dir_all(&work)?;
        fs::write(&executable, "#!/bin/sh\nprintf '%s' \"$1\" > \"$6/helper-started\"\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
        let mut updater = Updater {
            installed: Some(installed.clone()),
            status: UpdateStatus::Ready("0.9.7".into()),
            pending: None,
            release: None,
            prepared: Some(Prepared {
                version: Version::new(0, 9, 7),
                app: root.join(".Lizard-update-test.app"),
                destination: installed,
                work: work.clone(),
            }),
            last_check: None,
        };
        updater.install()?;
        let marker = work.join("helper-started");
        for _ in 0..250 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(marker)?, HELPER_ARG);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
