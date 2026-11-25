use crate::{bootstrap, config, jobs::Event};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, mpsc::Sender},
    thread,
    time::Duration,
};

const API: &str = "https://api.github.com/repos/Supa-Doge/supa-dev/releases/latest";
#[derive(Clone, Debug)]
pub struct Update {
    pub version: String,
    pub path: Option<PathBuf>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    url: String,
    #[serde(default)]
    digest: Option<String>,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
    draft: bool,
    prerelease: bool,
}
fn asset_name() -> &'static str {
    if cfg!(windows) {
        "supa-dev-windows-x86_64.exe"
    } else {
        "supa-dev-linux-x86_64"
    }
}
fn get(
    client: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    binary: bool,
) -> Result<reqwest::blocking::Response> {
    anyhow::ensure!(
        url.starts_with("https://api.github.com/"),
        "Некорректный адрес GitHub API"
    );
    let mut request = client.get(url).header(
        "Accept",
        if binary {
            "application/octet-stream"
        } else {
            "application/vnd.github+json"
        },
    );
    if !token.is_empty() {
        request = request.bearer_auth(token);
    }
    let response = request.send().context("GitHub недоступен")?;
    if matches!(response.status().as_u16(), 401 | 403 | 404) {
        anyhow::bail!(
            "Приватный релиз недоступен. Выполните gh auth login или укажите GitHub-токен с доступом к Supa-Doge/supa-dev."
        );
    }
    anyhow::ensure!(
        response.status().is_success(),
        "GitHub: HTTP {}",
        response.status()
    );
    Ok(response)
}
fn checksum(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sum = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| sum.to_ascii_lowercase())
    })
}
pub fn check(
    secrets: &config::Secrets,
    download: bool,
    tx: &Sender<Event>,
    cancel: &AtomicBool,
) -> Result<()> {
    let client = bootstrap::client()?;
    let token = config::github_token(secrets);
    let release: Release = get(&client, API, &token, false)?
        .json()
        .context("Некорректное описание релиза")?;
    let version = semver::Version::parse(release.tag_name.trim_start_matches('v'))?;
    anyhow::ensure!(
        !release.draft && !release.prerelease,
        "Ожидался стабильный релиз"
    );
    if version <= semver::Version::parse(env!("CARGO_PKG_VERSION"))? {
        let _ = tx.send(Event::UpdateStatus("Установлена актуальная версия".into()));
        return Ok(());
    }
    if !download {
        let _ = tx.send(Event::Update(Update {
            version: version.to_string(),
            path: None,
        }));
        return Ok(());
    }
    let name = asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .context("В релизе нет сборки для этой системы")?;
    let expected = if let Some(sum) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
    {
        sum.to_string()
    } else {
        let sums = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS")
            .context("В релизе отсутствует SHA256SUMS")?;
        checksum(&get(&client, &sums.url, &token, true)?.text()?, name)
            .context("Нет контрольной суммы приложения")?
    };
    let path = config::home()
        .join("updates")
        .join(version.to_string())
        .join(name);
    bootstrap::download(
        get(&client, &asset.url, &token, true)?,
        &path,
        "Обновление Supa Dev",
        tx,
        cancel,
    )?;
    anyhow::ensure!(
        bootstrap::sha256(&path)? == expected.to_ascii_lowercase(),
        "SHA-256 обновления не совпал"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    let _ = tx.send(Event::Update(Update {
        version: version.to_string(),
        path: Some(path),
    }));
    Ok(())
}
pub fn restart(update: &Path) -> Result<()> {
    let target = std::env::current_exe()?;
    let helper = config::home().join("updates").join(if cfg!(windows) {
        "update-helper.exe"
    } else {
        "update-helper"
    });
    fs::create_dir_all(helper.parent().unwrap())?;
    fs::copy(&target, &helper)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700))?;
    }
    config::command(helper)
        .arg("--apply-update")
        .arg(&target)
        .arg(update)
        .arg(std::process::id().to_string())
        .spawn()?;
    Ok(())
}
pub fn apply(target: &Path, source: &Path, pid: u32) -> Result<()> {
    anyhow::ensure!(
        target.is_file() && source.is_file(),
        "Файлы обновления отсутствуют"
    );
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            let result = WaitForSingleObject(process, 30000);
            CloseHandle(process);
            anyhow::ensure!(result == 0, "Приложение ещё работает");
        }
    }
    #[cfg(unix)]
    {
        for _ in 0..300 {
            if !Path::new(&format!("/proc/{pid}")).exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        anyhow::ensure!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "Приложение ещё работает"
        );
    }
    let backup = target.with_extension("previous");
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    for attempt in 0..30 {
        match fs::rename(target, &backup) {
            Ok(()) => break,
            Err(_) if attempt < 29 => thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.into()),
        }
    }
    if let Err(error) = fs::copy(source, target) {
        let _ = fs::rename(&backup, target);
        return Err(error.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(target, fs::Permissions::from_mode(0o700))?;
    }
    config::command(target).spawn()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_matches_exact_asset_only() {
        let sum = "a".repeat(64);
        let text = format!("{sum}  app.exe\n{}  app.exe.evil", "b".repeat(64));
        assert_eq!(checksum(&text, "app.exe"), Some(sum));
        assert_eq!(checksum(&text, "missing"), None);
    }
    #[test]
    fn old_or_equal_versions_do_not_update() {
        assert!(
            semver::Version::parse("0.1.0").unwrap()
                <= semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()
        );
    }
}
