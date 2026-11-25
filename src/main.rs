#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bootstrap;
mod config;
mod jobs;
mod updater;

use eframe::egui::{self, Color32, RichText};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const ACCENT: Color32 = Color32::from_rgb(174, 237, 125);
const MUTED: Color32 = Color32::from_rgb(150, 159, 169);

struct App {
    settings: config::Settings,
    secrets: config::Secrets,
    project: String,
    page: usize,
    busy: bool,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<jobs::Event>,
    rx: mpsc::Receiver<jobs::Event>,
    progress: (String, f32),
    status: serde_json::Value,
    logs: Vec<String>,
    error: Option<String>,
    update: Option<updater::Update>,
    update_message: String,
    next_update: Instant,
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(18, 22, 27);
        style.visuals.window_fill = Color32::from_rgb(25, 30, 36);
        style.visuals.selection.bg_fill = Color32::from_rgb(57, 79, 49);
        style.spacing.item_spacing = egui::vec2(12., 12.);
        style.spacing.button_padding = egui::vec2(16., 10.);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.));
        cc.egui_ctx.set_style(style);
        let (settings, secrets) = config::load();
        let (tx, rx) = mpsc::channel();
        Self {
            project: settings.workspace.clone(),
            page: if secrets.qwen.is_empty() { 1 } else { 0 },
            settings,
            secrets,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            progress: ("Готов к настройке".into(), 0.),
            status: serde_json::Value::Null,
            logs: vec![],
            error: None,
            update: None,
            update_message: "GitHub Releases · Supa-Doge/supa-dev".into(),
            next_update: Instant::now() + Duration::from_secs(15),
        }
    }
    fn log(&mut self, value: String) {
        // Neither credentials nor HTTP request bodies belong in the UI log.
        let mut clean = value;
        for secret in [&self.secrets.qwen, &self.secrets.github] {
            if !secret.is_empty() {
                clean = clean.replace(secret, "[ключ скрыт]");
            }
        }
        self.logs.push(clean);
        if self.logs.len() > 1000 {
            self.logs.drain(..200);
        }
    }
    fn start(&mut self, action: jobs::Action) {
        if self.busy {
            return;
        }
        self.error = None;
        self.busy = true;
        self.progress = ("Выполняется…".into(), 0.);
        self.cancel = Arc::new(AtomicBool::new(false));
        jobs::start(
            action,
            self.settings.clone(),
            self.secrets.clone(),
            self.tx.clone(),
            self.cancel.clone(),
        );
    }
    fn save(&mut self) {
        match config::save(&self.settings, &self.secrets) {
            Ok(()) => {
                self.log("Настройки сохранены на этом компьютере.".into());
                self.progress.0 = "Настройки сохранены".into();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    fn card(ui: &mut egui::Ui, title: &str, detail: &str, ready: Option<bool>) {
        egui::Frame::new()
            .fill(Color32::from_rgb(28, 33, 40))
            .corner_radius(12)
            .inner_margin(18)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 36.).max(100.));
                ui.horizontal(|ui| {
                    let color = match ready {
                        Some(true) => ACCENT,
                        Some(false) => Color32::from_rgb(229, 181, 105),
                        None => MUTED,
                    };
                    ui.colored_label(color, "●");
                    ui.label(RichText::new(title).size(17.).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(match ready {
                                Some(true) => "ГОТОВО",
                                Some(false) => "НУЖНА НАСТРОЙКА",
                                None => "НЕ ПРОВЕРЕНО",
                            })
                            .size(11.)
                            .color(color),
                        );
                    });
                });
                ui.add(egui::Label::new(RichText::new(detail).size(13.).color(MUTED)).truncate())
                    .on_hover_text(detail);
            });
    }
    fn overview(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Рабочее окружение").size(30.).strong());
        ui.label(
            RichText::new(
                "Облачный Qwen для задач. Локальный Qwen для правки отклонённых промптов.",
            )
            .color(MUTED),
        );
        ui.add_space(8.);
        let components = [
            (
                "qwen",
                "Qwen Code",
                "Официальная standalone-версия с Node.js",
            ),
            (
                "lmstudio",
                "LM Studio",
                "Использует установленную LM Studio или устанавливает llmster",
            ),
            (
                "model",
                "Локальный редактор",
                "Huihui Qwen3.8 27B · abliterated · Q4_K · 16.8 GB",
            ),
            (
                "gateway",
                "Гейтвей",
                "HTTP 400 → правка текста → повторный запрос в облако",
            ),
            (
                "python",
                "Встроенный Python",
                "Устанавливается автоматически; системный Python не нужен",
            ),
            (
                "alias",
                "Команда qwen-work",
                "Все команды Qwen, включая /model, доступны",
            ),
        ];
        egui::Grid::new("components")
            .num_columns(2)
            .spacing([12., 12.])
            .show(ui, |ui| {
                for (index, (key, name, fallback)) in components.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.set_width(
                            (ui.available_width() / if index % 2 == 0 { 2. } else { 1. } - 6.)
                                .max(290.),
                        );
                        Self::card(
                            ui,
                            name,
                            self.status[*key]["detail"].as_str().unwrap_or(fallback),
                            self.status[*key]["ready"].as_bool(),
                        );
                    });
                    if index % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(8.);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new(
                        RichText::new("Установить / восстановить").color(Color32::BLACK),
                    )
                    .fill(ACCENT),
                )
                .clicked()
            {
                self.start(jobs::Action::Install);
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Проверить состояние"))
                .clicked()
            {
                self.start(jobs::Action::Inspect);
            }
        });
        ui.label(RichText::new("Уже установленные компоненты используются повторно. Модель загружается в память при первой правке промпта.").size(13.).color(MUTED));
        ui.separator();
        ui.label(RichText::new("Открыть проект").size(20.).strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.project)
                .desired_width(f32::INFINITY)
                .hint_text("Путь к папке проекта"),
        );
        if ui
            .add_enabled(
                !self.busy,
                egui::Button::new("Запустить Qwen в терминале  →"),
            )
            .clicked()
        {
            self.start(jobs::Action::Launch(self.project.clone()));
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Настройка").size(30.).strong());
        ui.label("QwenCloud · Token Plan API Key");
        ui.add(
            egui::TextEdit::singleline(&mut self.secrets.qwen)
                .password(true)
                .desired_width(f32::INFINITY)
                .hint_text("Вставьте свой ключ Qwen"),
        );
        if ui
            .small_button("Подхватить ключ из установленного Qwen")
            .clicked()
        {
            match config::import_qwen_key() {
                Some(key) => {
                    self.secrets.qwen = key;
                    self.log("Ключ импортирован. Сохраните настройки.".into());
                }
                None => self.error = Some("Сохранённый ключ Qwen не найден.".into()),
            }
        }
        ui.label(
            RichText::new(if cfg!(windows) {
                "Ключ хранится локально, зашифрован средствами Windows для вашего пользователя."
            } else {
                "Ключ хранится локально: каталог 0700, файл 0600."
            })
            .size(13.)
            .color(MUTED),
        );
        ui.separator();
        ui.label("Рабочая папка и кэш");
        ui.add(
            egui::TextEdit::singleline(&mut self.settings.workspace).desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.label("Облачная модель");
            ui.add(egui::TextEdit::singleline(&mut self.settings.cloud_model).desired_width(240.));
            ui.label("Порт гейтвея");
            ui.add(egui::DragValue::new(&mut self.settings.gateway_port).range(1025..=65535));
        });
        ui.checkbox(
            &mut self.settings.yolo,
            "Запускать с --approval-mode=yolo (как в командной инструкции)",
        );
        ui.separator();
        ui.label("GitHub-токен для приватных релизов · необязательно при gh auth login");
        ui.add(
            egui::TextEdit::singleline(&mut self.secrets.github)
                .password(true)
                .desired_width(f32::INFINITY)
                .hint_text("Токен с доступом Contents: read к Supa-Doge/supa-dev"),
        );
        ui.checkbox(
            &mut self.settings.auto_updates,
            "Автоматически проверять и скачивать новые версии приложения",
        );
        ui.label(RichText::new("Обновление применяется после нажатия «Перезапустить». Текущие терминалы продолжают работать.").size(13.).color(MUTED));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new("Сохранить").fill(Color32::from_rgb(57, 79, 49)),
                )
                .clicked()
            {
                self.save();
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Применить и создать алиас"))
                .clicked()
            {
                self.start(jobs::Action::Configure);
            }
        });
        ui.add_space(8.);
        ui.label(
            RichText::new(format!("Данные приложения: {}", config::home().display()))
                .size(12.)
                .color(MUTED),
        );
    }
    fn updates(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("Обновления").size(30.).strong());
        ui.label(format!("Установлена версия {}", env!("CARGO_PKG_VERSION")));
        ui.hyperlink_to(
            "Supa-Doge/supa-dev · GitHub Releases",
            "https://github.com/Supa-Doge/supa-dev/releases",
        );
        ui.label(&self.update_message);
        if ui
            .add_enabled(
                !self.busy,
                egui::Button::new("Проверить и скачать обновление"),
            )
            .clicked()
        {
            self.start(jobs::Action::CheckUpdate(true));
        }
        if let Some(update) = self.update.clone() {
            ui.label(RichText::new(format!("Доступна версия {}", update.version)).color(ACCENT));
            if let Some(path) = update.path {
                if ui
                    .add_enabled(
                        !self.busy,
                        egui::Button::new("Перезапустить и обновить")
                            .fill(Color32::from_rgb(57, 79, 49)),
                    )
                    .clicked()
                {
                    match updater::restart(&path) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                jobs::Event::Log(value) => self.log(value),
                jobs::Event::Progress(text, value) => {
                    if self.progress.0 != text && !text.contains(" MB") {
                        self.log(text.clone());
                    }
                    self.progress = (text, value);
                }
                jobs::Event::Status(value) => self.status = value,
                jobs::Event::Error(value) => {
                    self.log(value.clone());
                    self.error = Some(value);
                }
                jobs::Event::Done => {
                    self.busy = false;
                    if self.error.is_none() {
                        self.progress = ("Готово".into(), 1.);
                    }
                }
                jobs::Event::Update(value) => {
                    self.update_message = "Обновление скачано и проверено по SHA-256".into();
                    self.update = Some(value);
                }
                jobs::Event::UpdateStatus(value) => self.update_message = value,
            }
        }
        if !self.busy && self.settings.auto_updates && Instant::now() >= self.next_update {
            self.next_update = Instant::now() + Duration::from_secs(6 * 3600);
            self.start(jobs::Action::CheckUpdate(true));
        }
        egui::TopBottomPanel::bottom("progress").show(ctx, |ui| {
            ui.add_space(6.);
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.progress.0);
                if self.busy && ui.small_button("Отменить").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
                if self.update.is_some() && ui.small_button("Доступно обновление").clicked()
                {
                    self.page = 2;
                }
            });
            if self.busy {
                ui.add(
                    egui::ProgressBar::new(self.progress.1.clamp(0., 1.))
                        .fill(ACCENT)
                        .animate(true),
                );
            }
            ui.add_space(6.);
        });
        egui::SidePanel::left("nav")
            .exact_width(176.)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(20.);
                ui.label(RichText::new("SUPA DEV").size(24.).color(ACCENT).strong());
                ui.label(
                    RichText::new(format!("v{} · Qwen workspace", env!("CARGO_PKG_VERSION")))
                        .size(11.)
                        .color(MUTED),
                );
                ui.add_space(28.);
                for (index, label) in ["Окружение", "Настройка", "Обновления", "Журнал"]
                    .iter()
                    .enumerate()
                {
                    if ui.selectable_label(self.page == index, *label).clicked() {
                        self.page = index;
                    }
                }
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .inner_margin(24)
                    .fill(Color32::from_rgb(18, 22, 27)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(error) = self.error.clone() {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(66, 32, 34))
                            .inner_margin(12)
                            .corner_radius(8)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(&error).color(Color32::from_rgb(255, 179, 177)),
                                );
                                if ui.small_button("Закрыть сообщение").clicked() {
                                    self.error = None;
                                }
                            });
                        ui.add_space(8.);
                    }
                    match self.page {
                        0 => self.overview(ui),
                        1 => self.settings(ui),
                        2 => self.updates(ui, ctx),
                        _ => {
                            ui.label(RichText::new("Журнал").size(30.).strong());
                            if ui.button("Скопировать журнал").clicked() {
                                ctx.copy_text(self.logs.join("\n"));
                            }
                            for line in &self.logs {
                                ui.label(RichText::new(line).monospace().size(13.));
                            }
                        }
                    }
                });
            });
        ctx.request_repaint_after(Duration::from_millis(if self.busy { 100 } else { 1000 }));
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--apply-update") => {
            anyhow::ensure!(args.len() == 5, "update arguments");
            return updater::apply(
                std::path::Path::new(&args[2]),
                std::path::Path::new(&args[3]),
                args[4].parse()?,
            );
        }
        Some("--version") => {
            println!("supa-dev {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--diagnose" | "--bootstrap") => {
            let (tx, rx) = mpsc::channel();
            let logger = std::thread::spawn(move || {
                for event in rx {
                    eprintln!("{event:?}");
                }
            });
            let python = bootstrap::ensure(&tx, &AtomicBool::new(false))?;
            drop(tx);
            let _ = logger.join();
            if args[1] == "--diagnose" {
                let output = config::command(python)
                    .arg(config::home().join("runtime/backend/scripts/manage.py"))
                    .arg("status")
                    .env("SUPA_DEV_HOME", config::home())
                    .env("PYTHONUTF8", "1")
                    .output()?;
                print!("{}", String::from_utf8_lossy(&output.stdout));
                anyhow::ensure!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return Ok(());
        }
        Some(other) => anyhow::bail!("Unknown argument: {other}"),
        None => {}
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120., 800.])
            .with_min_inner_size([940., 680.]),
        ..Default::default()
    };
    eframe::run_native(
        "Supa Dev",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        let _ = config::private_write(
            &config::home().join("last-error.log"),
            format!("{error:#}").as_bytes(),
        );
        std::process::exit(1);
    }
}
