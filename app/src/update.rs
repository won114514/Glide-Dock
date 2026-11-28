//! Background release checks and verified installer downloads.

use std::ffi::c_void;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dock_platform_windows::registry::Key;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{PCWSTR, w};

use crate::app::WM_APP_SETTINGS_COMMAND;

const GITHUB_REPOSITORY: &str = "won114514/Glide-Dock";
const CNB_REPOSITORY: &str = "won114514/Glide-Dock";
const GITHUB_RELEASES: &str = "https://api.github.com/repos/won114514/Glide-Dock/releases";
const CNB_RELEASES_PAGE: &str = "https://cnb.cool/won114514/Glide-Dock/-/releases";
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const HTTP_TIMEOUT_MS: i32 = 10_000;
const HTTP_TIMEOUTS_MS: [i32; 4] = [HTTP_TIMEOUT_MS; 4];
const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: usize = 1024;
const MAX_INSTALLER_BYTES: usize = 128 * 1024 * 1024;
const INSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Glide-Dock";
const LEGACY_INSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C6F1D7A-4B23-4D8E-9C1A-6E2F0A7B3C51}_is1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Source {
    Cnb,
    Github,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Cnb => "CNB",
            Self::Github => "GitHub",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Cnb => Self::Github,
            Self::Github => Self::Cnb,
        }
    }

    fn latest_url(self) -> String {
        match self {
            Self::Cnb => CNB_RELEASES_PAGE.into(),
            Self::Github => format!("{GITHUB_RELEASES}/latest"),
        }
    }

    fn asset_url(self, tag: &str, name: &str) -> String {
        match self {
            Self::Cnb => {
                format!("https://cnb.cool/{CNB_REPOSITORY}/-/releases/download/{tag}/{name}")
            }
            Self::Github => {
                format!("https://github.com/{GITHUB_REPOSITORY}/releases/download/{tag}/{name}")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct UpdateSnapshot {
    pub managed: bool,
    pub checking: bool,
    pub downloading: bool,
    pub ready: bool,
    pub launching: bool,
    pub progress: i32,
    pub status: String,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub source: String,
    pub cnb_status: String,
    pub github_status: String,
    pub last_check_text: String,
    last_checked: u64,
    release_tag: String,
    sha256: String,
}

impl Default for UpdateSnapshot {
    fn default() -> Self {
        Self {
            managed: false,
            checking: false,
            downloading: false,
            ready: false,
            launching: false,
            progress: 0,
            status: "可从 GitHub 或 CNB 发布页获取更新。".into(),
            current_version: env!("CARGO_PKG_VERSION").into(),
            latest_version: String::new(),
            release_notes: String::new(),
            source: String::new(),
            cnb_status: "尚未检查".into(),
            github_status: "尚未检查".into(),
            last_check_text: "尚未检查".into(),
            last_checked: 0,
            release_tag: String::new(),
            sha256: String::new(),
        }
    }
}

enum WorkerCommand {
    CheckNow,
    LaunchUpdate,
}

/// A UI-safe handle to the background update worker.
#[derive(Clone)]
pub(super) struct UpdateService {
    snapshot: Arc<Mutex<UpdateSnapshot>>,
    commands: Sender<WorkerCommand>,
}

impl UpdateService {
    pub(super) fn new(enabled: bool, owner: isize) -> Self {
        let managed = enabled && registered_running_install().is_some();
        let mut initial = load_snapshot();
        initial.managed = managed;
        initial.current_version = env!("CARGO_PKG_VERSION").into();
        if !managed {
            initial.ready = false;
            initial.checking = false;
            initial.downloading = false;
            initial.status = "便携运行模式不会覆盖更新；请从 GitHub 或 CNB 发布页下载新版。".into();
        } else if initial.ready && verified_cached_setup(&initial).is_err() {
            initial.ready = false;
            initial.status = "已缓存的更新文件不可用，请重新检查更新。".into();
        } else if initial.last_checked == 0 {
            initial.status = "尚未检查更新；可立即检查 CNB 与 GitHub。".into();
        }
        let snapshot = Arc::new(Mutex::new(initial));
        let (commands, receiver) = mpsc::channel();
        let worker_snapshot = Arc::clone(&snapshot);
        let state_path = state_path()
            .unwrap_or_else(|| std::env::temp_dir().join("Glide-Dock-update-state.json"));
        thread::Builder::new()
            .name("glide-update-worker".into())
            .spawn(move || update_worker(receiver, worker_snapshot, state_path, managed, owner))
            .expect("could not start Glide-Dock update worker");
        Self { snapshot, commands }
    }

    pub(super) fn snapshot(&self) -> UpdateSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub(super) fn check_now(&self) {
        if self.snapshot().managed {
            let _ = self.commands.send(WorkerCommand::CheckNow);
        }
    }

    pub(super) fn launch_update(&self) {
        if self.snapshot().ready {
            let _ = self.commands.send(WorkerCommand::LaunchUpdate);
        }
    }
}

#[derive(Debug, Clone)]
struct ReleaseCandidate {
    source: Source,
    version: Version,
    tag: String,
    notes: String,
    elapsed: Duration,
}

fn available_candidates(
    cnb: Result<Option<ReleaseCandidate>, (Source, String)>,
    github: Result<Option<ReleaseCandidate>, (Source, String)>,
) -> Vec<ReleaseCandidate> {
    [cnb.ok().flatten(), github.ok().flatten()]
        .into_iter()
        .flatten()
        .collect()
}

fn candidates_for_latest(
    mut candidates: Vec<ReleaseCandidate>,
    current: &Version,
) -> Vec<ReleaseCandidate> {
    let Some(latest) = candidates
        .iter()
        .map(|candidate| candidate.version.clone())
        .max()
    else {
        return Vec::new();
    };
    if latest <= *current {
        return Vec::new();
    }
    candidates.retain(|candidate| candidate.version == latest);
    candidates.sort_by_key(|candidate| candidate.elapsed);
    candidates
}

fn add_origin_fallback(candidates: &mut Vec<ReleaseCandidate>, version: Version, tag: &str) {
    let Some(preferred) = candidates.first() else {
        return;
    };
    let alternate = preferred.source.other();
    if !candidates
        .iter()
        .any(|candidate| candidate.source == alternate)
    {
        candidates.push(ReleaseCandidate {
            source: alternate,
            version,
            tag: tag.to_owned(),
            notes: String::new(),
            elapsed: Duration::MAX,
        });
    }
}

fn download_with_fallback(
    candidates: &[ReleaseCandidate],
    mut download: impl FnMut(&ReleaseCandidate) -> Result<String, String>,
) -> Result<(&ReleaseCandidate, String), Vec<String>> {
    let mut failures = Vec::new();
    for candidate in candidates {
        match download(candidate) {
            Ok(hash) => return Ok((candidate, hash)),
            Err(error) => failures.push(format!("{}：{error}", candidate.source.label())),
        }
    }
    Err(failures)
}

fn update_worker(
    receiver: Receiver<WorkerCommand>,
    snapshot: Arc<Mutex<UpdateSnapshot>>,
    path: PathBuf,
    managed: bool,
    owner: isize,
) {
    if managed {
        let last_checked = snapshot.lock().unwrap().last_checked;
        if should_check(last_checked, unix_now()) {
            check_and_download(&snapshot, &path, owner);
            notify_owner(owner);
        }
    }

    loop {
        match receiver.recv_timeout(CHECK_INTERVAL) {
            Ok(WorkerCommand::CheckNow) if managed => {
                check_and_download(&snapshot, &path, owner);
                notify_owner(owner);
            }
            Ok(WorkerCommand::LaunchUpdate) if managed => {
                launch_cached_update(&snapshot, owner);
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if managed => {
                check_and_download(&snapshot, &path, owner);
                notify_owner(owner);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn check_and_download(snapshot: &Arc<Mutex<UpdateSnapshot>>, state_file: &Path, owner: isize) {
    {
        let mut state = snapshot.lock().unwrap();
        if state.checking || state.downloading || state.launching {
            return;
        }
        state.checking = true;
        state.ready = false;
        state.progress = 0;
        state.status = "正在同时检查 CNB 与 GitHub…".into();
        state.latest_version.clear();
        state.release_notes.clear();
        state.source.clear();
    }
    notify_owner(owner);

    let (cnb, github) = check_latest_releases();
    let mut source_status = [String::new(), String::new()];
    for (source, result) in [(Source::Cnb, &cnb), (Source::Github, &github)] {
        let message = match result {
            Ok(Some(candidate)) => {
                format!("{}：v{} 可用", candidate.source.label(), candidate.version)
            }
            Ok(None) => format!("{}：未发现稳定版本", source.label()),
            Err((_, error)) => format!("{}：检查失败（{error}）", source.label()),
        };
        source_status[source_index(source)] = message;
    }

    let both_sources_failed = cnb.is_err() && github.is_err();
    let candidates = available_candidates(cnb, github);
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is valid");
    let newest = candidates
        .iter()
        .map(|candidate| candidate.version.clone())
        .max();
    let mut candidates = candidates_for_latest(candidates, &current);
    let latest = candidates
        .first()
        .map(|candidate| candidate.version.clone());

    {
        let mut state = snapshot.lock().unwrap();
        state.checking = false;
        state.last_checked = unix_now();
        state.last_check_text = "刚刚".into();
        state.cnb_status = source_status[0].clone();
        state.github_status = source_status[1].clone();
        if latest.is_none() {
            state.ready = false;
            state.downloading = false;
            state.status = if newest.is_some() {
                format!("当前已是最新版本 v{current}。")
            } else if both_sources_failed {
                "两个发布源暂时都不可用，请稍后重试。".into()
            } else {
                format!("未找到可用的稳定更新版本（当前 v{current}）。")
            };
            save_snapshot(state_file, &state);
            drop(state);
            notify_owner(owner);
            return;
        }
    }
    notify_owner(owner);

    let latest = latest.unwrap();
    let tag = candidates[0].tag.clone();
    // Asset names are deterministic for a stable tag. Try the other release
    // origin even if its metadata endpoint is unavailable; the expected SHA-256
    // asset must still validate the downloaded installer before it is cached.
    add_origin_fallback(&mut candidates, latest.clone(), &tag);
    candidates.sort_by_key(|candidate| candidate.elapsed);

    {
        let mut state = snapshot.lock().unwrap();
        state.downloading = true;
        state.latest_version = latest.to_string();
        state.release_tag = tag.clone();
        state.release_notes = candidates
            .iter()
            .find(|candidate| candidate.source == candidates[0].source)
            .map_or_else(String::new, |candidate| candidate.notes.clone());
        state.status = format!("发现 v{latest}，正在下载并验证安装包…");
    }
    notify_owner(owner);

    let result = download_with_fallback(&candidates, |candidate| {
        set_source_status(snapshot, candidate.source, "正在下载…");
        let mut last_progress = -1;
        let result = download_verified(candidate, snapshot, owner, &mut last_progress);
        if result.is_err() {
            set_source_status(snapshot, candidate.source, "下载或校验失败");
            notify_owner(owner);
        }
        result
    });
    match result {
        Ok((candidate, hash)) => {
            let mut state = snapshot.lock().unwrap();
            state.downloading = false;
            state.ready = true;
            state.progress = 100;
            state.source = candidate.source.label().into();
            state.sha256 = hash;
            state.status = format!("v{latest} 已下载并校验完成，确认后即可安装。");
            set_source_status_locked(&mut state, candidate.source, "下载并校验成功");
            save_snapshot(state_file, &state);
        }
        Err(failures) => {
            let mut state = snapshot.lock().unwrap();
            state.downloading = false;
            state.ready = false;
            state.status = format!(
                "新版 v{latest} 下载失败：{}。请重试或打开发布页手动下载。",
                failures.join("；")
            );
            save_snapshot(state_file, &state);
        }
    }
    notify_owner(owner);
}

type ReleaseResult = Result<Option<ReleaseCandidate>, (Source, String)>;

fn check_latest_releases() -> (ReleaseResult, ReleaseResult) {
    thread::scope(|scope| {
        let cnb = scope.spawn(|| query_latest_release(Source::Cnb));
        let github = scope.spawn(|| query_latest_release(Source::Github));
        let cnb = cnb
            .join()
            .unwrap_or_else(|_| Err((Source::Cnb, "检查线程异常退出".into())));
        let github = github
            .join()
            .unwrap_or_else(|_| Err((Source::Github, "检查线程异常退出".into())));
        (cnb, github)
    })
}

fn query_latest_release(source: Source) -> Result<Option<ReleaseCandidate>, (Source, String)> {
    let started = Instant::now();
    let bytes =
        http_get(&source.latest_url(), MAX_METADATA_BYTES).map_err(|error| (source, error))?;
    parse_latest_candidates(source, &bytes, started.elapsed()).map_err(|error| (source, error))
}

fn parse_latest_candidates(
    source: Source,
    bytes: &[u8],
    elapsed: Duration,
) -> Result<Option<ReleaseCandidate>, String> {
    if source == Source::Cnb && bytes.windows(5).any(|window| window == b"<html") {
        return parse_cnb_releases_page(bytes, elapsed);
    }
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| format!("发布信息格式无效：{error}"))?;
    let data = root.get("data").unwrap_or(&root);
    let releases = root
        .as_array()
        .or_else(|| root.get("releases").and_then(serde_json::Value::as_array))
        .or_else(|| data.as_array())
        .or_else(|| data.get("releases").and_then(serde_json::Value::as_array));
    let Some(releases) = releases else {
        return parse_candidate(source, bytes, elapsed);
    };
    let mut candidates = releases
        .iter()
        .filter_map(|release| {
            serde_json::to_vec(release)
                .ok()
                .and_then(|bytes| parse_candidate(source, &bytes, elapsed).ok().flatten())
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.version.cmp(&left.version));
    Ok(candidates.into_iter().next())
}

fn parse_cnb_releases_page(
    bytes: &[u8],
    elapsed: Duration,
) -> Result<Option<ReleaseCandidate>, String> {
    let html =
        std::str::from_utf8(bytes).map_err(|error| format!("CNB 发布页面编码无效：{error}"))?;
    let marker = "id=\"__NEXT_DATA__\"";
    let start = html
        .find(marker)
        .and_then(|index| html[index..].find('>').map(|end| index + end + 1))
        .ok_or_else(|| "CNB 发布页面缺少版本数据".to_string())?;
    let end = html[start..]
        .find("</script>")
        .map(|index| start + index)
        .ok_or_else(|| "CNB 发布页面版本数据不完整".to_string())?;
    let root: serde_json::Value = serde_json::from_str(&html[start..end])
        .map_err(|error| format!("CNB 发布页面数据无效：{error}"))?;
    let releases =
        find_cnb_releases(&root).ok_or_else(|| "CNB 发布页面未包含 Release 列表".to_string())?;
    let mut candidates = releases
        .iter()
        .filter_map(|release| {
            let tag = release.get("tag_ref").and_then(serde_json::Value::as_str)?;
            if release.get("is_draft").and_then(serde_json::Value::as_bool) == Some(true)
                || release
                    .get("is_prerelease")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                || stable_release_version(tag.rsplit('/').next()?).is_none()
            {
                return None;
            }
            let normalized = serde_json::json!({
                "tag_name": tag.rsplit('/').next()?,
                "body": release.get("body").and_then(serde_json::Value::as_str).unwrap_or_default(),
                "assets": release.get("assets").and_then(serde_json::Value::as_array)
                    .map(|assets| assets.iter().map(|asset| serde_json::json!({
                        "name": asset.get("name")
                    })).collect::<Vec<_>>()).unwrap_or_default()
            });
            parse_candidate(Source::Cnb, normalized.to_string().as_bytes(), elapsed)
                .ok()
                .flatten()
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.version.cmp(&left.version));
    Ok(candidates.into_iter().next())
}

fn find_cnb_releases(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    match value {
        serde_json::Value::Object(object) => object.iter().find_map(|(key, value)| {
            if key == "releases" && value.is_array() {
                value.as_array()
            } else {
                find_cnb_releases(value)
            }
        }),
        serde_json::Value::Array(values) => values.iter().find_map(find_cnb_releases),
        _ => None,
    }
}

fn parse_candidate(
    source: Source,
    bytes: &[u8],
    elapsed: Duration,
) -> Result<Option<ReleaseCandidate>, String> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| format!("发布信息格式无效：{error}"))?;
    let release = root.get("data").unwrap_or(&root);
    let release = release.get("release").unwrap_or(release);
    let tag = release
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "发布信息缺少版本标签".to_string())?;
    if release
        .get("draft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || release
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    {
        return Ok(None);
    }
    let Some(version) = stable_release_version(tag) else {
        return Ok(None);
    };
    let expected_exe = format!("Glide-Dock-{tag}-Setup.exe");
    let expected_checksum = format!("{expected_exe}.sha256");
    let assets = release
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "发布信息缺少安装包附件".to_string())?;
    if !assets.iter().any(|asset| {
        asset.get("name").and_then(serde_json::Value::as_str) == Some(expected_exe.as_str())
    }) || !assets.iter().any(|asset| {
        asset.get("name").and_then(serde_json::Value::as_str) == Some(expected_checksum.as_str())
    }) {
        return Err("发布版缺少预期的安装包或 SHA-256 文件".into());
    }
    let notes = release
        .get("body")
        .or_else(|| release.get("description"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(6000)
        .collect();
    Ok(Some(ReleaseCandidate {
        source,
        version,
        tag: tag.to_string(),
        notes,
        elapsed,
    }))
}

fn stable_release_version(tag: &str) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix('v')?).ok()?;
    if !version.pre.is_empty() || !version.build.is_empty() || tag != format!("v{version}") {
        return None;
    }
    Some(version)
}

fn download_verified(
    candidate: &ReleaseCandidate,
    snapshot: &Arc<Mutex<UpdateSnapshot>>,
    owner: isize,
    last_progress: &mut i32,
) -> Result<String, String> {
    let directory = updates_directory().ok_or_else(|| "无法确定本机更新缓存目录".to_string())?;
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建更新缓存目录：{error}"))?;
    let installer_name = format!("Glide-Dock-{}-Setup.exe", candidate.tag);
    let checksum_name = format!("{installer_name}.sha256");
    let installer_path = directory.join(&installer_name);
    let partial_path = directory.join(format!("{installer_name}.download"));
    let checksum_path = directory.join(&checksum_name);

    let checksum = http_get(
        &candidate.source.asset_url(&candidate.tag, &checksum_name),
        MAX_CHECKSUM_BYTES,
    )?;
    let expected = parse_sha256(&checksum, &installer_name)
        .ok_or_else(|| "发布源提供的 SHA-256 文件格式错误或文件名不匹配".to_string())?;
    let mut bytes = http_get_with_progress(
        &candidate.source.asset_url(&candidate.tag, &installer_name),
        MAX_INSTALLER_BYTES,
        |received, total| {
            let progress = received
                .saturating_mul(100)
                .checked_div(total)
                .map_or(0, |percent| percent.min(99) as i32);
            if progress != *last_progress {
                *last_progress = progress;
                snapshot.lock().unwrap().progress = progress;
                notify_owner(owner);
            }
        },
    )?;
    let actual = sha256(&bytes);
    if !sha256_matches(&actual, &expected) {
        bytes.fill(0);
        let _ = fs::remove_file(&partial_path);
        return Err("安装包 SHA-256 校验失败".into());
    }

    {
        let mut file = fs::File::create(&partial_path)
            .map_err(|error| format!("无法保存已验证的更新：{error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("无法写入更新文件：{error}"))?;
    }
    bytes.fill(0);
    fs::rename(&partial_path, &installer_path)
        .or_else(|_| {
            let _ = fs::remove_file(&installer_path);
            fs::rename(&partial_path, &installer_path)
        })
        .map_err(|error| format!("无法完成更新文件保存：{error}"))?;
    fs::write(&checksum_path, checksum).map_err(|error| format!("无法保存校验记录：{error}"))?;
    Ok(actual)
}

fn parse_sha256(content: &[u8], expected_name: &str) -> Option<String> {
    let text = std::str::from_utf8(content).ok()?.trim();
    let mut fields = text.split_whitespace();
    let hash = fields.next()?;
    let file_name = fields.next()?.trim_start_matches('*');
    if fields.next().is_some()
        || hash.len() != 64
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || file_name != expected_name
    {
        return None;
    }
    Some(hash.to_ascii_lowercase())
}

fn sha256(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

fn sha256_matches(actual: &str, expected: &str) -> bool {
    actual.len() == 64
        && expected.len() == 64
        && actual.bytes().all(|byte| byte.is_ascii_hexdigit())
        && expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        && actual.eq_ignore_ascii_case(expected)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn launch_cached_update(snapshot: &Arc<Mutex<UpdateSnapshot>>, owner: isize) {
    let (path, expected_hash) = {
        let mut state = snapshot.lock().unwrap();
        if !state.ready || state.launching {
            return;
        }
        state.launching = true;
        state.status = "正在启动已校验的更新程序…".into();
        match verified_cached_setup(&state) {
            Ok(path) => (path, state.sha256.clone()),
            Err(error) => {
                state.launching = false;
                state.ready = false;
                state.status = format!("更新文件验证失败：{error}。请重新检查更新。",);
                drop(state);
                notify_owner(owner);
                return;
            }
        }
    };

    if !file_sha256(&path).is_ok_and(|hash| sha256_matches(&hash, &expected_hash)) {
        let mut state = snapshot.lock().unwrap();
        state.launching = false;
        state.ready = false;
        state.status = "更新文件发生变化，已阻止启动；请重新检查更新。".into();
        drop(state);
        notify_owner(owner);
        return;
    }
    notify_owner(owner);
    let result = std::process::Command::new(&path)
        .arg("--auto-update")
        .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
        .spawn();
    let mut state = snapshot.lock().unwrap();
    state.launching = false;
    match result {
        Ok(_) => state.status = "更新程序已启动；请完成可能出现的 Windows 管理员确认。".into(),
        Err(error) => state.status = format!("无法启动更新程序：{error}。请重试。"),
    }
    drop(state);
    notify_owner(owner);
}

fn verified_cached_setup(state: &UpdateSnapshot) -> Result<PathBuf, String> {
    let version =
        stable_release_version(&state.release_tag).ok_or_else(|| "更新版本标签无效".to_string())?;
    if version.to_string() != state.latest_version {
        return Err("缓存的更新版本信息不一致".into());
    }
    let directory = updates_directory().ok_or_else(|| "更新缓存目录不可用".to_string())?;
    let path = directory.join(format!("Glide-Dock-{}-Setup.exe", state.release_tag));
    if !path.is_file() {
        return Err("缓存中找不到安装程序".into());
    }
    Ok(path)
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex_lower(&digest.finalize()))
}

fn load_snapshot() -> UpdateSnapshot {
    let mut snapshot: UpdateSnapshot = state_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if snapshot.last_checked > 0 {
        snapshot.last_check_text = "此前检查完成".into();
    }
    snapshot
}

fn save_snapshot(path: &Path, state: &UpdateSnapshot) {
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(state) else {
        return;
    };
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if fs::write(&temporary, bytes).is_ok() && fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(path);
        let _ = fs::rename(&temporary, path);
    }
}

fn state_path() -> Option<PathBuf> {
    Some(updates_directory()?.join("state.json"))
}

fn updates_directory() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    if local.is_empty() {
        return None;
    }
    Some(PathBuf::from(local).join("Glide-Dock").join("Updates"))
}

fn registered_running_install() -> Option<String> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let roots = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER];
    for root in roots {
        for key_path in [INSTALL_KEY, LEGACY_INSTALL_KEY] {
            let Ok(Some(key)) = Key::open(root, key_path, KEY_QUERY_VALUE) else {
                continue;
            };
            let Ok(Some(directory)) = key.text("InstallLocation") else {
                continue;
            };
            let installed_exe = PathBuf::from(&directory).join("glide-dock.exe");
            if installed_exe
                .canonicalize()
                .is_ok_and(|registered| registered == executable)
                && key.text("DisplayVersion").ok().flatten().is_some()
            {
                return Some(directory);
            }
        }
    }
    None
}

fn should_check(last_checked: u64, now: u64) -> bool {
    last_checked == 0 || now < last_checked || now - last_checked >= CHECK_INTERVAL.as_secs()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn notify_owner(owner: isize) {
    if owner != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(windows::Win32::Foundation::HWND(owner as *mut _)),
                WM_APP_SETTINGS_COMMAND,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }
}

fn source_index(source: Source) -> usize {
    match source {
        Source::Cnb => 0,
        Source::Github => 1,
    }
}

fn set_source_status(snapshot: &Arc<Mutex<UpdateSnapshot>>, source: Source, message: &str) {
    set_source_status_locked(&mut snapshot.lock().unwrap(), source, message);
}

fn set_source_status_locked(snapshot: &mut UpdateSnapshot, source: Source, message: &str) {
    let status = format!("{}：{message}", source.label());
    match source {
        Source::Cnb => snapshot.cnb_status = status,
        Source::Github => snapshot.github_status = status,
    }
}

struct WinHttpHandle(*mut c_void);

impl WinHttpHandle {
    fn new(handle: *mut c_void, kind: &str) -> Result<Self, String> {
        if handle.is_null() {
            Err(format!("无法创建 HTTPS {kind}"))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for WinHttpHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn http_get(url: &str, maximum: usize) -> Result<Vec<u8>, String> {
    http_get_with_progress(url, maximum, |_, _| {})
}

fn http_get_with_progress(
    url: &str,
    maximum: usize,
    mut progress: impl FnMut(u64, u64),
) -> Result<Vec<u8>, String> {
    let (host, path) = split_https_url(url)?;
    let agent = wide(&format!("Glide-Dock/{}", env!("CARGO_PKG_VERSION")));
    let host = wide(host);
    let path = wide(path);
    let session = WinHttpHandle::new(
        unsafe {
            WinHttpOpen(
                PCWSTR::from_raw(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        },
        "会话",
    )?;
    unsafe {
        WinHttpSetTimeouts(
            session.0,
            HTTP_TIMEOUTS_MS[0],
            HTTP_TIMEOUTS_MS[1],
            HTTP_TIMEOUTS_MS[2],
            HTTP_TIMEOUTS_MS[3],
        )
        .map_err(|error| format!("HTTPS 超时设置失败：{error}"))?;
    }
    let connection = WinHttpHandle::new(
        unsafe {
            WinHttpConnect(
                session.0,
                PCWSTR::from_raw(host.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        },
        "连接",
    )?;
    let request = WinHttpHandle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR::from_raw(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        },
        "请求",
    )?;
    let redirect_policy = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP.to_ne_bytes();
    unsafe {
        WinHttpSetOption(
            Some(request.0),
            WINHTTP_OPTION_REDIRECT_POLICY,
            Some(&redirect_policy),
        )
        .map_err(|error| format!("无法限制 HTTPS 重定向策略：{error}"))?;
    }
    let headers = if host_name(url) == "api.github.com" {
        "Accept: application/vnd.github+json\r\nUser-Agent: Glide-Dock\r\nAccept-Encoding: identity\r\n"
    } else if host_name(url) == "cnb.cool" {
        "Accept: text/html\r\nAccept-Encoding: identity\r\n"
    } else {
        "Accept: application/octet-stream\r\nAccept-Encoding: identity\r\n"
    }
    .encode_utf16()
    .collect::<Vec<_>>();
    unsafe {
        WinHttpAddRequestHeaders(
            request.0,
            &headers,
            WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
        )
        .map_err(|error| format!("无法设置下载请求头：{error}"))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)
            .map_err(|error| format!("HTTPS 请求失败：{error}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|error| format!("HTTPS 响应失败：{error}"))?;
    }
    let mut status_code = 0u32;
    let mut status_bytes = std::mem::size_of::<u32>() as u32;
    let mut header_index = 0u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status_code as *mut u32).cast()),
            &mut status_bytes,
            &mut header_index,
        )
        .map_err(|error| format!("无法读取 HTTP 状态：{error}"))?;
    }
    if status_code != 200 {
        return Err(format!("服务器返回 HTTP {status_code}"));
    }
    validate_final_url(request.0)?;

    let mut total = 0u32;
    let mut total_bytes = std::mem::size_of::<u32>() as u32;
    let mut total_index = 0u32;
    let total = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut total as *mut u32).cast()),
            &mut total_bytes,
            &mut total_index,
        )
        .map(|()| total as u64)
        .unwrap_or(0)
    };
    if total > maximum as u64 {
        return Err("下载文件超过允许大小".into());
    }

    let mut data = Vec::with_capacity((total as usize).min(maximum));
    let mut received = 0u64;
    loop {
        let mut available = 0u32;
        unsafe {
            WinHttpQueryDataAvailable(request.0, &mut available)
                .map_err(|error| format!("读取下载数据失败：{error}"))?;
        }
        if available == 0 {
            break;
        }
        if data.len().saturating_add(available as usize) > maximum {
            return Err("下载文件超过允许大小".into());
        }
        let start = data.len();
        data.resize(start + available as usize, 0);
        let mut read = 0u32;
        unsafe {
            WinHttpReadData(
                request.0,
                data[start..].as_mut_ptr().cast(),
                available,
                &mut read,
            )
            .map_err(|error| format!("读取下载数据失败：{error}"))?;
        }
        data.truncate(start + read as usize);
        received += read as u64;
        progress(received, total);
    }
    if total > 0 && received != total {
        return Err("下载内容长度与服务器声明不一致".into());
    }
    Ok(data)
}

fn split_https_url(url: &str) -> Result<(&str, &str), String> {
    let remainder = url
        .strip_prefix("https://")
        .ok_or_else(|| "只允许 HTTPS 更新源".to_string())?;
    let (host, path) = remainder
        .split_once('/')
        .ok_or_else(|| "更新地址无效".to_string())?;
    if !is_allowed_host(host) || path.contains('#') || path.contains(' ') {
        return Err("更新地址不在允许的发布域名中".into());
    }
    Ok((host, &remainder[host.len()..]))
}

fn is_allowed_host(host: &str) -> bool {
    matches!(
        host,
        "api.github.com"
            | "github.com"
            | "api.cnb.cool"
            | "cnb.cool"
            | "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com"
            | "cdn.cnb.cool"
            | "assets.cnb.cool"
            | "download.cnb.cool"
            | "cnbcdn.com"
    ) || host.ends_with(".cnbcdn.com")
}

fn validate_final_url(request: *mut c_void) -> Result<(), String> {
    let mut length = 0u32;
    unsafe {
        let _ = WinHttpQueryOption(request, WINHTTP_OPTION_URL, None, &mut length);
    }
    if !(4..=32 * 1024).contains(&length) {
        return Err("无法验证 HTTPS 下载的最终地址".into());
    }
    let mut buffer = vec![0u16; (length as usize).div_ceil(2)];
    unsafe {
        WinHttpQueryOption(
            request,
            WINHTTP_OPTION_URL,
            Some(buffer.as_mut_ptr().cast()),
            &mut length,
        )
        .map_err(|_| "无法验证 HTTPS 下载的最终地址".to_string())?;
    }
    let url = String::from_utf16_lossy(&buffer);
    let url = url.trim_end_matches('\0');
    split_https_url(url).map(|_| ())
}

fn host_name(url: &str) -> &str {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split_once('/').map(|(host, _)| host))
        .unwrap_or_default()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(tag: &str, draft: bool, prerelease: bool) -> Vec<u8> {
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": prerelease,
            "body": "release notes",
            "assets": [
                { "name": format!("Glide-Dock-{tag}-Setup.exe") },
                { "name": format!("Glide-Dock-{tag}-Setup.exe.sha256") },
            ],
        })
        .to_string()
        .into_bytes()
    }

    fn candidate(source: Source, version: &str, elapsed_ms: u64) -> ReleaseCandidate {
        let version = Version::parse(version).unwrap();
        ReleaseCandidate {
            source,
            tag: format!("v{version}"),
            version,
            notes: String::new(),
            elapsed: Duration::from_millis(elapsed_ms),
        }
    }

    #[test]
    fn one_source_timeout_does_not_hide_the_other_source_release() {
        let candidates = available_candidates(
            Err((Source::Cnb, "模拟请求超时".into())),
            Ok(Some(candidate(Source::Github, "1.3.0", 100))),
        );
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].source, Source::Github);
    }

    #[test]
    fn selects_newest_version_without_downgrading_and_prefers_faster_equal_source() {
        let candidates = vec![
            candidate(Source::Github, "1.2.0", 20),
            candidate(Source::Cnb, "1.3.0", 40),
            candidate(Source::Github, "1.3.0", 10),
        ];
        let selected = candidates_for_latest(candidates.clone(), &Version::new(1, 2, 9));
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].source, Source::Github);
        assert_eq!(selected[0].version, Version::new(1, 3, 0));
        assert!(candidates_for_latest(candidates, &Version::new(1, 3, 0)).is_empty());
    }

    #[test]
    fn download_failure_falls_back_to_other_source() {
        let mut candidates = vec![candidate(Source::Cnb, "1.3.0", 5)];
        add_origin_fallback(&mut candidates, Version::new(1, 3, 0), "v1.3.0");
        assert_eq!(candidates[1].source, Source::Github);
        let mut attempted = Vec::new();
        let result = download_with_fallback(&candidates, |item| {
            attempted.push(item.source);
            if item.source == Source::Cnb {
                Err("模拟下载超时".into())
            } else {
                Ok("verified".into())
            }
        })
        .unwrap();
        assert_eq!(attempted, [Source::Cnb, Source::Github]);
        assert_eq!(result.0.source, Source::Github);
        assert_eq!(result.1, "verified");
    }

    #[test]
    fn http_timeout_is_bounded_and_checksum_mismatch_is_rejected() {
        assert_eq!(HTTP_TIMEOUTS_MS, [10_000; 4]);
        let expected = sha256(b"known installer bytes");
        assert!(sha256_matches(&expected, &expected.to_ascii_uppercase()));
        assert!(!sha256_matches(
            &expected,
            &sha256(b"different installer bytes")
        ));
        assert!(!sha256_matches("bad", &expected));
    }

    #[test]
    fn accepts_only_stable_semver_release_tags() {
        assert_eq!(
            stable_release_version("v1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
        for tag in ["1.2.3", "v1.2", "v1.2.3-beta.1", "v01.2.3", "v1.2.3+meta"] {
            assert!(stable_release_version(tag).is_none(), "{tag}");
        }
    }

    #[test]
    fn parses_release_only_when_installer_and_checksum_are_present() {
        let candidate = parse_candidate(
            Source::Github,
            &release_json("v1.2.3", false, false),
            Duration::from_millis(10),
        )
        .unwrap()
        .unwrap();
        assert_eq!(candidate.version, Version::new(1, 2, 3));
        assert_eq!(candidate.notes, "release notes");
        assert!(
            parse_candidate(
                Source::Github,
                &release_json("v1.2.3-beta.1", false, true),
                Duration::ZERO
            )
            .unwrap()
            .is_none()
        );
        let missing_asset = serde_json::json!({"tag_name":"v1.2.3", "assets":[]}).to_string();
        assert!(parse_candidate(Source::Github, missing_asset.as_bytes(), Duration::ZERO).is_err());
    }

    #[test]
    fn latest_release_list_selects_highest_valid_stable_version() {
        let releases = serde_json::json!([
            serde_json::from_slice::<serde_json::Value>(&release_json("v1.2.3", false, false))
                .unwrap(),
            serde_json::from_slice::<serde_json::Value>(&release_json(
                "v1.4.0-beta.1",
                false,
                true
            ))
            .unwrap(),
            serde_json::from_slice::<serde_json::Value>(&release_json("v1.3.0", false, false))
                .unwrap(),
        ]);
        let latest =
            parse_latest_candidates(Source::Cnb, releases.to_string().as_bytes(), Duration::ZERO)
                .unwrap()
                .unwrap();
        assert_eq!(latest.version, Version::new(1, 3, 0));
    }

    #[test]
    fn cnb_release_list_can_be_read_from_data_wrapper() {
        let release: serde_json::Value =
            serde_json::from_slice(&release_json("v1.2.3", false, false)).unwrap();
        let wrapped = serde_json::json!({"data": {"releases": [release]}});
        let latest =
            parse_latest_candidates(Source::Cnb, wrapped.to_string().as_bytes(), Duration::ZERO)
                .unwrap()
                .unwrap();
        assert_eq!(latest.version, Version::new(1, 2, 3));
    }

    #[test]
    fn public_cnb_release_page_selects_latest_published_stable_release() {
        let html = format!(
            r#"<html><script nonce="test" id="__NEXT_DATA__" type="application/json">{}</script></html>"#,
            serde_json::json!({"props": {"pageProps": {"initialState": {"releasesInfo": {"releases": [
                {"tag_ref": "refs/tags/v1.2.0", "is_draft": false, "is_prerelease": false,
                 "body": "old notes", "assets": []},
                {"tag_ref": "refs/tags/v1.3.0", "is_draft": false, "is_prerelease": false,
                 "body": "latest notes", "assets": [
                    {"name": "Glide-Dock-v1.3.0-Setup.exe"},
                    {"name": "Glide-Dock-v1.3.0-Setup.exe.sha256"}
                 ]},
                {"tag_ref": "refs/tags/v1.4.0-beta.1", "is_draft": false, "is_prerelease": true,
                 "body": "preview", "assets": []},
                {"tag_ref": "refs/tags/v1.5.0", "is_draft": true, "is_prerelease": false,
                 "body": "draft", "assets": []}
            ]}}}}})
        );
        let latest = parse_latest_candidates(Source::Cnb, html.as_bytes(), Duration::ZERO)
            .unwrap()
            .unwrap();
        assert_eq!(latest.version, Version::new(1, 3, 0));
        assert_eq!(latest.tag, "v1.3.0");
        assert_eq!(latest.notes, "latest notes");
    }

    #[test]
    fn checksum_must_match_filename_and_sha256_shape() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_sha256(format!("{hash}  setup.exe\n").as_bytes(), "setup.exe"),
            Some(hash.clone())
        );
        assert_eq!(
            parse_sha256(format!("{hash}  other.exe\n").as_bytes(), "setup.exe"),
            None
        );
        assert_eq!(parse_sha256(b"deadbeef setup.exe", "setup.exe"), None);
    }

    #[test]
    fn auto_check_is_throttled_for_twenty_four_hours() {
        let interval = CHECK_INTERVAL.as_secs();
        assert!(should_check(0, 500));
        assert!(!should_check(1_000, 1_000 + interval - 1));
        assert!(should_check(1_000, 1_000 + interval));
        assert!(should_check(2_000, 1_000));
    }

    #[test]
    fn source_urls_are_https_and_point_to_expected_release_paths() {
        assert_eq!(
            split_https_url(&Source::Cnb.latest_url()).unwrap(),
            ("cnb.cool", "/won114514/Glide-Dock/-/releases")
        );
        assert_eq!(
            split_https_url(&Source::Github.latest_url()).unwrap(),
            (
                "api.github.com",
                "/repos/won114514/Glide-Dock/releases/latest"
            )
        );
        assert_eq!(
            split_https_url(&Source::Cnb.asset_url("v1.2.3", "file.exe")).unwrap(),
            (
                "cnb.cool",
                "/won114514/Glide-Dock/-/releases/download/v1.2.3/file.exe"
            )
        );
        assert_eq!(
            split_https_url(&Source::Github.asset_url("v1.2.3", "file.exe")).unwrap(),
            (
                "github.com",
                "/won114514/Glide-Dock/releases/download/v1.2.3/file.exe"
            )
        );
        assert!(split_https_url("http://github.com/evil.exe").is_err());
        assert!(split_https_url("https://attacker.invalid/file.exe").is_err());
    }
}
