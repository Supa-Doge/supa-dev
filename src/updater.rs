use crate::{bootstrap, config, jobs::Event};
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, mpsc::Sender},
    thread,
    time::Duration,
};

const RELEASES: &str = "https://github.com/Supa-Doge/supa-dev/releases";
#[derive(Clone, Debug)]
pub struct Update {
    pub version: String,
    pub path: Option<PathBuf>,
}
fn asset_name() -> &'static str {
    if cfg!(windows) {
        "supa-dev-windows-x86_64.exe"
    } else {
        "supa-dev-linux-x86_64"
    }
}
fn get(client: &reqwest::blocking::Client, url: &str) -> Result<reqwest::blocking::Response> {
    anyhow::ensure!(
        url.starts_with(&format!("{RELEASES}/download/")),
        "Некорректный адрес релиза"
    );
    let response = client
        .get(url)
        .send()
        .context("Сервер обновлений недоступен")?;
    anyhow::ensure!(
        response.status().is_success(),
        "GitHub: HTTP {}",
        response.status()
    );
    Ok(response)
}
fn release_version(url: &str) -> Result<(String, semver::Version)> {
    let tag = url
        .strip_prefix(&format!("{RELEASES}/tag/"))
        .context("Некорректный адрес последнего релиза")?;
    let version = semver::Version::parse(tag.trim_start_matches('v'))?;
    anyhow::ensure!(version.pre.is_empty(), "Ожидался стабильный релиз");
    Ok((tag.into(), version))
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
pub fn check(tx: &Sender<Event>, cancel: &AtomicBool) -> Result<()> {
    check_since(
        &semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        tx,
        cancel,
    )
}
pub fn download_latest(tx: &Sender<Event>, cancel: &AtomicBool) -> Result<()> {
    // Diagnostic download only; it never installs or restarts an application.
    check_since(&semver::Version::new(0, 0, 0), tx, cancel)
}
fn check_since(current: &semver::Version, tx: &Sender<Event>, cancel: &AtomicBool) -> Result<()> {
    let client = bootstrap::client()?;
    // Public web release links do not consume the REST API's shared-IP quota.
    let response = client
        .head(format!("{RELEASES}/latest"))
        .send()
        .context("Не удалось проверить обновление")?;
    anyhow::ensure!(
        response.status().is_success(),
        "Проверка обновления: HTTP {}",
        response.status()
    );
    let (tag, version) = release_version(response.url().as_str())?;
    if &version <= current {
        let _ = tx.send(Event::UpdateStatus("Установлена актуальная версия".into()));
        return Ok(());
    }
    let name = asset_name();
    let base = format!("{RELEASES}/download/{tag}");
    let expected = checksum(&get(&client, &format!("{base}/SHA256SUMS"))?.text()?, name)
        .context("Нет контрольной суммы приложения")?;
    let path = config::home()
        .join("updates")
        .join(version.to_string())
        .join(name);
    // Reuse a verified download after choosing "Later" or restarting the app.
    if !path.is_file() || bootstrap::sha256(&path)? != expected.to_ascii_lowercase() {
        bootstrap::download(
            get(&client, &format!("{base}/{name}"))?,
            &path,
            "Обновление Supa Dev",
            tx,
            cancel,
        )?;
    }
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
    #[test]
    fn release_redirect_must_point_to_this_repository_and_stable_version() {
        assert_eq!(
            release_version(&format!("{RELEASES}/tag/v0.1.2"))
                .unwrap()
                .1,
            semver::Version::new(0, 1, 2)
        );
        assert!(release_version("https://example.invalid/releases/tag/v9.0.0").is_err());
        assert!(release_version(&format!("{RELEASES}/tag/v0.1.2-beta.1")).is_err());
        assert!(release_version(&format!("{RELEASES}/tag/../../other")).is_err());
    }
}
