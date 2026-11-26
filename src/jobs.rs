use crate::{bootstrap, config, updater};
use std::{
    io::{BufRead, BufReader},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug)]
pub enum Event {
    Log(String),
    Progress(String, f32),
    Status(serde_json::Value),
    Error(String),
    Done(bool),
    Update(updater::Update),
    UpdateStatus(String),
}
#[derive(Clone)]
pub enum Action {
    Install,
    Inspect,
    Configure,
    StopLocal,
    ResumeLocal,
    Launch(String),
    CheckUpdate,
}

pub fn start(
    action: Action,
    settings: config::Settings,
    secrets: config::Secrets,
    tx: Sender<Event>,
    cancel: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let result = (|| -> anyhow::Result<()> {
            if let Action::CheckUpdate = action {
                return updater::check(&tx, &cancel);
            }
            config::save(&settings, &secrets)?;
            let python = bootstrap::ensure(&tx, &cancel)?;
            let backend = config::home().join("runtime/backend/scripts/manage.py");
            let (name, project) = match action {
                Action::Install => ("install", None),
                Action::Inspect => ("status", None),
                Action::Configure => ("configure", None),
                Action::StopLocal => ("stop-local", None),
                Action::ResumeLocal => ("resume-local", None),
                Action::Launch(folder) => ("launch", Some(folder)),
                _ => unreachable!(),
            };
            let mut command = config::command(python);
            command
                .arg("-u")
                .arg(backend)
                .arg(name)
                .env("SUPA_DEV_HOME", config::home())
                .env("PYTHONUTF8", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(project) = project {
                command.arg("--project").arg(project);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            let mut child = command.spawn()?;
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let sender = tx.clone();
            let reader = thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                        let message = value["message"].as_str().unwrap_or_default().to_string();
                        let event = match value["type"].as_str().unwrap_or_default() {
                            "progress" => Some(Event::Progress(
                                message,
                                value["progress"].as_f64().unwrap_or(0.) as f32,
                            )),
                            "status" => Some(Event::Status(value["components"].clone())),
                            "error" => Some(Event::Error(message)),
                            "log" => Some(Event::Log(message)),
                            _ => None,
                        };
                        if let Some(event) = event {
                            let _ = sender.send(event);
                        }
                    } else if !line.trim().is_empty() {
                        let _ = sender.send(Event::Log(line));
                    }
                }
            });
            let sender = tx.clone();
            let errors = thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = sender.send(Event::Log(line));
                }
            });
            let status = loop {
                if cancel.load(Ordering::Relaxed) {
                    #[cfg(windows)]
                    {
                        let _ = config::command("taskkill.exe")
                            .args(["/PID", &child.id().to_string(), "/T", "/F"])
                            .output();
                    }
                    #[cfg(unix)]
                    {
                        let _ = config::command("kill")
                            .args(["-TERM", "--", &format!("-{}", child.id())])
                            .output();
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("Операция отменена. Скачанные части модели сохранены.");
                }
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                thread::sleep(Duration::from_millis(120));
            };
            let _ = reader.join();
            let _ = errors.join();
            anyhow::ensure!(status.success(), "Операция не завершена; подробности выше.");
            Ok(())
        })();
        let success = result.is_ok();
        if let Err(error) = result {
            let _ = tx.send(Event::Error(format!("{error:#}")));
        }
        let _ = tx.send(Event::Done(success));
    });
}
