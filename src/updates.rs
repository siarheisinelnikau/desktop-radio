use crate::{http::Response, model::data_dir};
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    ptr::null,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    Storage::FileSystem::ReplaceFileW,
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

const REPOSITORY: &str = "siarheisinelnikau/desktop-radio";
const MAX_EXE: u64 = 100 * 1024 * 1024;

#[derive(Clone)]
pub struct Release {
    pub version: Version,
    executable: Asset,
    checksum: Asset,
}

#[derive(Deserialize)]
struct ResponseRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
}

pub enum Check {
    Current,
    NoRelease,
    New(Release),
}

pub fn check() -> Result<Check> {
    let url = Url::parse(&format!(
        "https://api.github.com/repos/{REPOSITORY}/releases/latest"
    ))?;
    let response = match Response::open(&url, "Accept: application/vnd.github+json\r\n", 7000) {
        Ok(response) => response,
        Err(error) if error.to_string() == "HTTP 404" => return Ok(Check::NoRelease),
        Err(error) => return Err(error),
    };
    let response: ResponseRelease = serde_json::from_slice(&response.collect(1024 * 1024)?)?;
    let version = Version::parse(response.tag_name.trim_start_matches('v'))?;
    if response.draft || response.prerelease || !version.pre.is_empty() {
        return Ok(Check::NoRelease);
    }
    if version <= Version::parse(env!("CARGO_PKG_VERSION"))? {
        return Ok(Check::Current);
    }
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let generic = format!("DesktopRadio-v{version}.exe");
    let specific = format!("DesktopRadio-v{version}-{arch}.exe");
    let Some(executable) = response
        .assets
        .iter()
        .find(|asset| asset.name == specific)
        .or_else(|| response.assets.iter().find(|asset| asset.name == generic))
    else {
        return Ok(Check::NoRelease);
    };
    let name = format!("{}.sha256", executable.name);
    let Some(checksum) = response.assets.iter().find(|asset| asset.name == name) else {
        return Ok(Check::NoRelease);
    };
    for asset in [executable, checksum] {
        let url = Url::parse(&asset.browser_download_url)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str() == Some("github.com")
                && url.path()
                    == format!(
                        "/{REPOSITORY}/releases/download/{}/{}",
                        response.tag_name, asset.name
                    ),
            "Release asset is outside the official repository"
        );
    }
    ensure!(
        (1..=MAX_EXE).contains(&executable.size) && (64..=1024).contains(&checksum.size),
        "Invalid release asset size"
    );
    Ok(Check::New(Release {
        version,
        executable: executable.clone(),
        checksum: checksum.clone(),
    }))
}

fn open(asset: &Asset) -> Result<Response> {
    let url = Url::parse(&asset.browser_download_url)?;
    let response = Response::open(&url, "", 12_000)?;
    let actual = response.effective_url(&url);
    ensure!(
        actual.scheme() == "https"
            && matches!(
                actual.host_str(),
                Some(
                    "github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            ),
        "Release asset redirected outside GitHub"
    );
    Ok(response)
}

pub fn download(release: &Release, progress: impl Fn(u8)) -> Result<()> {
    let executable = std::env::current_exe()?;
    let checksum_bytes = open(&release.checksum)?.collect(1024)?;
    let checksum = std::str::from_utf8(&checksum_bytes)?
        .split_whitespace()
        .next()
        .context("Missing SHA-256 checksum")?
        .to_ascii_lowercase();
    ensure!(
        checksum.len() == 64 && checksum.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid SHA-256 checksum"
    );
    let key = format!(
        "{:x}-{:x}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let helper = helper_path(&key);
    let staged = sibling(&executable, &key, "pending")?;
    fs::create_dir_all(helper.parent().context("Invalid update helper path")?)?;
    let result = (|| -> Result<()> {
        let mut input = open(&release.executable)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        let mut buffer = [0u8; 64 * 1024];
        let mut algorithm = Sha256::new();
        let mut size = 0_u64;
        let mut last_percent = 0;
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            ensure!(
                size <= release.executable.size && size <= MAX_EXE,
                "Unexpected update size"
            );
            output.write_all(&buffer[..count])?;
            algorithm.update(&buffer[..count]);
            let percent = (size * 100 / release.executable.size) as u8;
            if percent != last_percent {
                last_percent = percent;
                progress(percent);
            }
        }
        ensure!(
            size == release.executable.size && format!("{:x}", algorithm.finalize()) == checksum,
            "Update checksum did not match"
        );
        output.sync_all()?;
        drop(output);
        verify(&staged, &checksum)?;
        fs::copy(&executable, &helper)?;
        Command::new(&helper)
            .arg("--apply-update")
            .arg(&executable)
            .arg(&key)
            .arg(std::process::id().to_string())
            .arg(checksum)
            .spawn()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
        let _ = fs::remove_file(helper);
    }
    result
}

fn verify(path: &Path, expected: &str) -> Result<()> {
    let data = fs::read(path)?;
    ensure!(
        (data.len() as u64) <= MAX_EXE && data.starts_with(b"MZ"),
        "Invalid Windows executable"
    );
    ensure!(
        format!("{:x}", Sha256::digest(&data)) == expected,
        "Staged update checksum did not match"
    );
    let offset = u32::from_le_bytes(
        data.get(60..64)
            .context("Incomplete PE header")?
            .try_into()?,
    ) as usize;
    let machine = if cfg!(target_arch = "aarch64") {
        0xaa64u16
    } else {
        0x8664u16
    }
    .to_le_bytes();
    ensure!(
        data.get(offset..offset.saturating_add(6))
            == Some(&[b'P', b'E', 0, 0, machine[0], machine[1]]),
        "Wrong executable architecture"
    );
    Ok(())
}

pub fn apply(installed: &Path, key: &str, pid: u32, hash: &str) -> Result<()> {
    ensure!(
        valid_key(key)
            && std::env::current_exe()?.canonicalize()? == helper_path(key).canonicalize()?
            && installed.is_absolute()
            && installed
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case("exe")),
        "Invalid update session"
    );
    let staged = sibling(installed, key, "pending")?;
    let backup = sibling(installed, key, "backup")?;
    let mut replaced = false;
    let result = (|| -> Result<()> {
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if !process.is_null() {
            let wait = unsafe { WaitForSingleObject(process, 20_000) };
            unsafe {
                CloseHandle(process);
            }
            ensure!(wait == WAIT_OBJECT_0, "Desktop Radio did not exit");
        }
        verify(&staged, hash)?;
        replace(installed, &staged, Some(&backup))?;
        replaced = true;
        Command::new(installed)
            .arg("--cleanup-update")
            .arg(key)
            .spawn()?;
        let _ = fs::remove_file(&backup);
        Ok(())
    })();
    if result.is_err() {
        if replaced {
            replace(installed, &backup, None)?;
        }
        let _ = fs::remove_file(staged);
        let _ = Command::new(installed)
            .arg("--update-failed")
            .arg(key)
            .spawn();
    }
    result
}

pub fn cleanup(key: &str) {
    if !valid_key(key) {
        return;
    }
    let filename = helper_path(key);
    std::thread::spawn(move || {
        for _ in 0..20 {
            if !filename.exists() || fs::remove_file(&filename).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

fn helper_path(key: &str) -> PathBuf {
    data_dir().join("Updates").join(format!("{key}.exe"))
}

fn valid_key(key: &str) -> bool {
    key.len() <= 48
        && key.split_once('-').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes()
                    .chain(b.bytes())
                    .all(|byte| byte.is_ascii_hexdigit())
        })
}

fn sibling(executable: &Path, key: &str, extension: &str) -> Result<PathBuf> {
    let filename = executable
        .file_name()
        .context("Missing executable name")?
        .to_string_lossy();
    Ok(executable.with_file_name(format!("{filename}.{key}.{extension}")))
}

fn replace(original: &Path, pending: &Path, backup: Option<&Path>) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let encode = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let original = encode(original);
    let pending = encode(pending);
    let backup = backup.map(encode);
    if unsafe {
        ReplaceFileW(
            original.as_ptr(),
            pending.as_ptr(),
            backup.as_ref().map_or(null(), |file| file.as_ptr()),
            0,
            null(),
            null(),
        )
    } == 0
    {
        bail!(std::io::Error::last_os_error());
    }
    Ok(())
}
