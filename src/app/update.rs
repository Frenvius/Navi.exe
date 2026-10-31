use serde::Deserialize;
use std::{
    io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};

const LATEST_URL: &str = "https://api.github.com/repos/Frenvius/Navi.exe/releases/latest";
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{7C1E4B52-9A3D-4F6E-8B21-5D0A9E3C6F14}_is1";
const SUMS_NAME: &str = "SHA256SUMS.txt";
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const API_LIMIT: usize = 1 << 20;
const SETUP_LIMIT: usize = 256 << 20;
const SETUP_ARGS: [&str; 5] = [
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/CLOSEAPPLICATIONS",
    "/RELAUNCH",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
        let mut next = || parts.next()?.parse().ok();
        let version = Self(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("package version is x.y.z")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    setup_name: String,
    setup_url: String,
    sums_url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Available(Release),
    Installing(Release),
    Launched,
    Failed(Release, String),
}

impl State {
    pub fn shows_button(&self) -> bool {
        matches!(
            self,
            State::Available(_) | State::Installing(_) | State::Failed(..)
        )
    }
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    assets: Vec<ApiAsset>,
}

fn setup_name(version: Version) -> String {
    format!("Navi-Setup-{version}.exe")
}

fn parse_release(json: &[u8]) -> io::Result<Release> {
    let release: ApiRelease = serde_json::from_slice(json).map_err(io::Error::other)?;
    let version = Version::parse(&release.tag_name)
        .ok_or_else(|| io::Error::other(format!("unexpected tag {}", release.tag_name)))?;
    let setup_name = setup_name(version);
    let url = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
            .ok_or_else(|| io::Error::other(format!("release has no {name}")))
    };
    Ok(Release {
        version,
        setup_url: url(&setup_name)?,
        sums_url: url(SUMS_NAME)?,
        setup_name,
    })
}

fn expected_hash(sums: &str, name: &str) -> Option<[u8; 32]> {
    let hex = sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start_matches([' ', '*']) == name).then_some(hash)
    })?;
    if hex.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}

fn sha256(bytes: &[u8]) -> io::Result<[u8; 32]> {
    let mut hash = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, bytes, &mut hash) }
        .ok()
        .map_err(io::Error::other)?;
    Ok(hash)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let normal = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches('\\')
            .to_lowercase()
    };
    normal(a) == normal(b)
}

fn simulation() -> Option<String> {
    #[cfg(debug_assertions)]
    {
        std::env::var("NAVI_UPDATE_TEST")
            .ok()
            .filter(|value| matches!(value.as_str(), "available" | "failure"))
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn installed() -> bool {
    let location = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(UNINSTALL_KEY)
        .and_then(|key| key.get_value::<String, _>("InstallLocation"));
    let current = std::env::current_exe().ok();
    match (location, current.as_deref().and_then(Path::parent)) {
        (Ok(location), Some(dir)) => same_dir(Path::new(&location), dir),
        _ => false,
    }
}

fn updates_dir() -> io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::other("LOCALAPPDATA is not set"))?;
    Ok(PathBuf::from(base).join("Navi").join("updates"))
}

fn remove_stale_setups() {
    let Ok(dir) = updates_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let current = Version::current();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let version = name
            .strip_prefix("Navi-Setup-")
            .and_then(|rest| rest.strip_suffix(".exe"))
            .and_then(Version::parse);
        if version.is_some_and(|version| version <= current) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn shared_state() -> &'static Mutex<State> {
    static STATE: Mutex<State> = Mutex::new(State::Idle);
    &STATE
}

fn set(state: State) {
    *shared_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = state;
}

pub fn state() -> State {
    shared_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn simulated_release() -> Release {
    Release {
        version: Version(99, 0, 0),
        setup_name: setup_name(Version(99, 0, 0)),
        setup_url: String::new(),
        sums_url: String::new(),
    }
}

fn check() -> io::Result<Option<Release>> {
    if simulation().is_some() {
        return Ok(Some(simulated_release()));
    }
    let json = crate::shell::http::get(LATEST_URL, "application/vnd.github+json", API_LIMIT)?;
    let release = parse_release(&json)?;
    Ok((release.version > Version::current()).then_some(release))
}

pub fn start_checks(notify: impl Fn() + Send + 'static) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() || (simulation().is_none() && !installed()) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            remove_stale_setups();
            loop {
                if let Ok(Some(release)) = check() {
                    let changed = {
                        let mut state = shared_state()
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        let replace = match &*state {
                            State::Idle => true,
                            State::Available(known) | State::Failed(known, _) => {
                                known.version < release.version
                            }
                            State::Installing(_) | State::Launched => false,
                        };
                        if replace {
                            *state = State::Available(release);
                        }
                        replace
                    };
                    if changed {
                        notify();
                    }
                }
                std::thread::sleep(CHECK_INTERVAL);
            }
        });
}

fn download_and_launch(release: &Release) -> io::Result<()> {
    if let Some(mode) = simulation() {
        std::thread::sleep(Duration::from_millis(1500));
        return match mode.as_str() {
            "failure" => Err(io::Error::other("simulated update failure")),
            _ => Err(io::Error::other("simulated release cannot be installed")),
        };
    }
    let sums = crate::shell::http::get(&release.sums_url, "application/octet-stream", API_LIMIT)?;
    let expected = expected_hash(&String::from_utf8_lossy(&sums), &release.setup_name)
        .ok_or_else(|| io::Error::other(format!("{SUMS_NAME} lists no {}", release.setup_name)))?;
    let setup = crate::shell::http::get(&release.setup_url, "application/octet-stream", SETUP_LIMIT)?;
    if sha256(&setup)? != expected {
        return Err(io::Error::other(format!(
            "{} does not match its SHA-256",
            release.setup_name
        )));
    }
    let dir = updates_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&release.setup_name);
    std::fs::write(&path, &setup)?;
    Command::new(&path)
        .args(SETUP_ARGS)
        .creation_flags(windows::Win32::System::Threading::DETACHED_PROCESS.0)
        .spawn()?;
    Ok(())
}

pub fn install(notify: impl Fn() + Send + 'static) -> bool {
    let release = {
        let mut state = shared_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (State::Available(release) | State::Failed(release, _)) = &*state else {
            return false;
        };
        let release = release.clone();
        *state = State::Installing(release.clone());
        release
    };
    let spawned = std::thread::Builder::new()
        .name("update-install".into())
        .spawn(move || {
            notify();
            set(match download_and_launch(&release) {
                Ok(()) => State::Launched,
                Err(error) => State::Failed(release, error.to_string()),
            });
            notify();
        });
    if let Err(error) = spawned {
        let State::Installing(release) = state() else {
            return false;
        };
        set(State::Failed(release, error.to_string()));
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_strictly_and_order_numerically() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("1.10.0"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1.2.3-beta"), None);
        assert!(Version(1, 10, 0) > Version(1, 9, 9));
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn release_json_needs_setup_and_checksums() {
        let json = br#"{"tag_name":"v1.2.0","draft":false,"assets":[
            {"name":"Navi-Setup-1.2.0.exe","browser_download_url":"https://x/setup","size":1},
            {"name":"SHA256SUMS.txt","browser_download_url":"https://x/sums"}]}"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, Version(1, 2, 0));
        assert_eq!(release.setup_name, "Navi-Setup-1.2.0.exe");
        assert_eq!(release.setup_url, "https://x/setup");
        assert_eq!(release.sums_url, "https://x/sums");
        let missing = br#"{"tag_name":"v1.2.0","assets":[
            {"name":"SHA256SUMS.txt","browser_download_url":"https://x/sums"}]}"#;
        assert!(parse_release(missing).is_err());
        assert!(parse_release(br#"{"tag_name":"nightly","assets":[]}"#).is_err());
    }

    #[test]
    fn checksums_match_by_file_name() {
        let hash = "ab".repeat(32);
        let sums = format!("{}  other.exe\n{hash}  Navi-Setup-1.2.0.exe\n", "00".repeat(32));
        assert_eq!(
            expected_hash(&sums, "Navi-Setup-1.2.0.exe"),
            Some([0xab; 32])
        );
        assert_eq!(expected_hash(&sums, "missing.exe"), None);
        assert_eq!(expected_hash("abc  Navi-Setup-1.2.0.exe", "Navi-Setup-1.2.0.exe"), None);
    }

    #[test]
    fn sha256_matches_known_digest() {
        let digest = sha256(b"abc").unwrap();
        assert_eq!(
            digest[..4],
            [0xba, 0x78, 0x16, 0xbf]
        );
    }

    #[test]
    fn install_dirs_compare_case_and_slash_insensitively() {
        assert!(same_dir(
            Path::new(r"C:\Users\x\AppData\Local\Programs\Navi\"),
            Path::new(r"c:\users\x\appdata\local\programs\navi")
        ));
        assert!(!same_dir(Path::new(r"C:\Navi"), Path::new(r"C:\Navi2")));
    }
}
