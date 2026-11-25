use crate::{config, jobs::Event};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};
include!(concat!(env!("OUT_DIR"), "/backend_assets.rs"));

pub fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("SupaDev/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .build()?)
}
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn download(
    mut response: reqwest::blocking::Response,
    path: &Path,
    title: &str,
    tx: &Sender<Event>,
    cancel: &AtomicBool,
) -> Result<()> {
    anyhow::ensure!(
        response.status().is_success(),
        "{title}: HTTP {}",
        response.status()
    );
    fs::create_dir_all(path.parent().context("download directory")?)?;
    let partial = path.with_extension("part");
    let total = response.content_length().unwrap_or(0);
    let mut file = fs::File::create(&partial)?;
    let mut buffer = vec![0; 256 * 1024];
    let mut done = 0_u64;
    let mut last = Instant::now();
    loop {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Загрузка отменена");
        let size = response
            .read(&mut buffer)
            .context("Соединение при загрузке прервалось")?;
        if size == 0 {
            break;
        }
        file.write_all(&buffer[..size])?;
        done += size as u64;
        if last.elapsed() > Duration::from_millis(150) {
            let _ = tx.send(Event::Progress(
                format!("{title}: {:.1} MB", done as f64 / 1e6),
                if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.
                },
            ));
            last = Instant::now();
        }
    }
    anyhow::ensure!(total == 0 || done == total, "Неполная загрузка {title}");
    file.sync_all()?;
    drop(file);
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(partial, path)?;
    Ok(())
}
pub fn python() -> PathBuf {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../assets/runtime-manifest.json"))
            .expect("runtime manifest");
    let version = manifest[if cfg!(windows) { "windows" } else { "linux" }]["version"]
        .as_str()
        .expect("runtime version");
    let root = config::home()
        .join("runtime")
        .join(format!("python-{version}"));
    if cfg!(windows) {
        root.join("python.exe")
    } else {
        root.join("python/bin/python3")
    }
}
pub fn ensure(tx: &Sender<Event>, cancel: &AtomicBool) -> Result<PathBuf> {
    let home = config::home();
    fs::create_dir_all(&home)?;
    let backend = home.join("runtime/backend");
    for (relative, bytes) in BACKEND {
        let path = backend.join(relative);
        if fs::read(&path).ok().as_deref() != Some(*bytes) {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, bytes)?;
        }
    }
    let python = python();
    if python.is_file() {
        return Ok(python);
    }
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../assets/runtime-manifest.json"))?;
    let spec = &manifest[if cfg!(windows) { "windows" } else { "linux" }];
    let url = spec["url"].as_str().context("runtime URL")?;
    let expected = spec["sha256"].as_str().context("runtime SHA256")?;
    let archive = home.join("downloads").join(if cfg!(windows) {
        "python.zip"
    } else {
        "python.tar.gz"
    });
    let _ = tx.send(Event::Progress("Установка встроенного Python".into(), 0.));
    if !archive.exists() || sha256(&archive)? != expected {
        download(
            client()?
                .get(url)
                .send()
                .context("Не удалось скачать Python")?,
            &archive,
            "Python",
            tx,
            cancel,
        )?;
    }
    anyhow::ensure!(
        sha256(&archive)? == expected,
        "Не совпала SHA-256 архива Python"
    );
    let destination = home.join("runtime").join(format!(
        "python-{}",
        spec["version"].as_str().context("runtime version")?
    ));
    fs::create_dir_all(&destination)?;
    if cfg!(windows) {
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive)?)?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            let relative = entry.enclosed_name().context("Небезопасный путь в ZIP")?;
            let output = destination.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&output)?;
            } else {
                fs::create_dir_all(output.parent().unwrap())?;
                std::io::copy(&mut entry, &mut fs::File::create(output)?)?;
            }
        }
    } else {
        let decoder = flate2::read::GzDecoder::new(fs::File::open(&archive)?);
        tar::Archive::new(decoder).unpack(&destination)?;
    }
    anyhow::ensure!(python.is_file(), "В архиве отсутствует Python");
    let check = config::command(&python)
        .args([
            "-c",
            "import ssl,sqlite3,ctypes,urllib.request;print('READY')",
        ])
        .output()?;
    anyhow::ensure!(check.status.success(), "Встроенный Python не запускается");
    let _ = tx.send(Event::Log(format!(
        "Python {} готов",
        spec["version"].as_str().unwrap_or_default()
    )));
    Ok(python)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_is_pinned() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../assets/runtime-manifest.json")).unwrap();
        for platform in ["windows", "linux"] {
            let sum = v[platform]["sha256"].as_str().unwrap();
            assert_eq!(sum.len(), 64);
            assert!(sum.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(v[platform]["url"].as_str().unwrap().starts_with("https://"));
        }
    }
    #[test]
    fn backend_has_no_generated_private_artifacts() {
        for (name, _) in BACKEND {
            assert!(!name.contains("secrets"));
            assert!(!name.contains("sessions"));
            assert!(!name.contains("temp/"));
        }
    }
}
