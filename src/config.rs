use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub workspace: String,
    pub cloud_model: String,
    pub gateway_port: u16,
    pub auto_updates: bool,
    pub yolo: bool,
}
impl Default for Settings {
    fn default() -> Self {
        let users = directories::UserDirs::new().expect("user directory");
        let desktop = users
            .desktop_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| users.home_dir().join("Desktop"));
        Self {
            workspace: desktop.join("qwen").to_string_lossy().into(),
            cloud_model: "qwen4-max".into(),
            gateway_port: 1236,
            auto_updates: true,
            yolo: true,
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Secrets {
    pub qwen: String,
    pub github: String,
}

pub fn home() -> PathBuf {
    std::env::var_os("SUPA_DEV_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::ProjectDirs::from("", "", "SupaDev")
                .expect("app directory")
                .data_local_dir()
                .into()
        })
}
pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("parent directory")?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        std::io::Write::write_all(&mut file, bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    fs::write(path, bytes)?;
    Ok(())
}
pub fn load() -> (Settings, Secrets) {
    let dir = home();
    let settings = fs::read(dir.join("config.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let secrets = fs::read(dir.join("secrets.bin"))
        .ok()
        .and_then(|v| unseal(&v).ok())
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    (settings, secrets)
}
pub fn save(settings: &Settings, secrets: &Secrets) -> Result<()> {
    anyhow::ensure!(
        settings.gateway_port > 1024 && settings.gateway_port != 1234,
        "Выберите порт шлюза выше 1024, отличный от 1234"
    );
    anyhow::ensure!(
        !settings.workspace.trim().is_empty(),
        "Укажите рабочую папку"
    );
    private_write(
        &home().join("config.json"),
        &serde_json::to_vec_pretty(settings)?,
    )?;
    private_write(
        &home().join("secrets.bin"),
        &seal(&serde_json::to_vec(secrets)?)?,
    )
}
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
pub fn github_token(secrets: &Secrets) -> String {
    if !secrets.github.trim().is_empty() {
        return secrets.github.trim().into();
    }
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    command("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
        .unwrap_or_default()
}
pub fn import_qwen_key() -> Option<String> {
    if let Ok(key) = std::env::var("QWENCLOUD_TOKEN_PLAN_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    let path = directories::UserDirs::new()?
        .home_dir()
        .join(".qwen/settings.json");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value
        .pointer("/env/QWENCLOUD_TOKEN_PLAN_API_KEY")?
        .as_str()
        .map(str::to_string)
}
#[cfg(windows)]
fn crypt(data: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len().try_into()?,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    anyhow::ensure!(
        ok != 0,
        "Не удалось открыть локальное хранилище ключей: {}",
        std::io::Error::last_os_error()
    );
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as _);
    }
    Ok(result)
}
#[cfg(windows)]
fn seal(data: &[u8]) -> Result<Vec<u8>> {
    crypt(data, true)
}
#[cfg(windows)]
fn unseal(data: &[u8]) -> Result<Vec<u8>> {
    crypt(data, false)
}
#[cfg(not(windows))]
fn seal(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}
#[cfg(not(windows))]
fn unseal(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_roundtrip() {
        let value = b"synthetic-only";
        assert_eq!(unseal(&seal(value).unwrap()).unwrap(), value);
    }
    #[test]
    fn defaults_keep_local_and_cloud_ports_separate() {
        assert_ne!(Settings::default().gateway_port, 1234);
    }
}
