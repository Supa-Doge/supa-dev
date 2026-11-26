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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WizardStep {
    Key,
    Workspace,
    Install,
    Ready,
}

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
    wizard: Option<WizardStep>,
    active_action: Option<jobs::Action>,
    update_tx: mpsc::Sender<jobs::Event>,
    update_rx: mpsc::Receiver<jobs::Event>,
    update_checking: bool,
    update_prompt: bool,
    postponed_update: Option<String>,
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
        Self::from_settings(settings, secrets)
    }
    fn from_settings(settings: config::Settings, secrets: config::Secrets) -> Self {
        let (tx, rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        Self {
            wizard: if settings.setup_completed && !secrets.qwen.trim().is_empty() {
                None
            } else {
                Some(WizardStep::Key)
            },
            active_action: None,
            update_tx,
            update_rx,
            update_checking: false,
            update_prompt: false,
            postponed_update: None,
            project: settings.workspace.clone(),
            page: if secrets.qwen.is_empty() { 1 } else { 0 },
            settings,
            secrets,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            progress: ("Готов к настройке".into(), 0.),
            status: serde_json::json!({"local_editor": {"enabled": config::editor_enabled()}}),
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
        for secret in [&self.secrets.qwen] {
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
        self.active_action = Some(action.clone());
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
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10., 10.), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4., color);
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
        ui.label(RichText::new("Модель загружается только для правки промпта и выгружается после неё. При простое — автоматическая выгрузка через 60 секунд.").size(13.).color(MUTED));
        let editor_enabled = self.status["local_editor"]["enabled"]
            .as_bool()
            .unwrap_or(true);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new("Остановить и выгрузить редактор"),
                )
                .clicked()
            {
                self.start(jobs::Action::StopLocal);
            }
            if !editor_enabled
                && ui
                    .add_enabled(!self.busy, egui::Button::new("Включить редактор"))
                    .clicked()
            {
                self.start(jobs::Action::ResumeLocal);
            }
        });
        if !editor_enabled {
            ui.label(RichText::new("Редактор остановлен. Облачный Qwen доступен; правка отклонённых промптов возобновится после включения редактора.").size(13.).color(MUTED));
        }
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
            Self::model_select(ui, &mut self.settings.cloud_model);
            ui.label("Порт гейтвея");
            ui.add(egui::DragValue::new(&mut self.settings.gateway_port).range(1025..=65535));
        });
        ui.checkbox(
            &mut self.settings.yolo,
            "Запускать с --approval-mode=yolo (как в командной инструкции)",
        );
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
                .add_enabled(!self.busy, egui::Button::new("Применить к Qwen"))
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
                !self.update_checking,
                egui::Button::new("Проверить и скачать обновление"),
            )
            .clicked()
        {
            self.check_updates();
        }
        if let Some(update) = self.update.clone() {
            ui.label(RichText::new(format!("Доступна версия {}", update.version)).color(ACCENT));
            if update.path.is_some() {
                if ui
                    .add_enabled(
                        !self.busy,
                        egui::Button::new("Обновиться").fill(Color32::from_rgb(57, 79, 49)),
                    )
                    .clicked()
                {
                    self.update_prompt = true;
                }
            }
        }
        let _ = ctx;
    }
    fn model_select(ui: &mut egui::Ui, selected: &mut String) {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../backend/config/gateway.json"))
                .expect("model catalog");
        egui::ComboBox::from_id_salt("cloud_model")
            .selected_text(selected.as_str())
            .width(240.)
            .show_ui(ui, |ui| {
                if let Some(models) = manifest["cloudModels"].as_array() {
                    for model in models.iter().filter_map(|item| item.as_str()) {
                        ui.selectable_value(selected, model.to_string(), model);
                    }
                }
            });
    }
    fn finish_setup(&mut self) {
        self.settings.setup_completed = true;
        match config::save(&self.settings, &self.secrets) {
            Ok(()) => {
                self.wizard = None;
                self.page = 0;
                self.error = None;
                self.next_update = Instant::now() + Duration::from_secs(2);
            }
            Err(error) => {
                self.settings.setup_completed = false;
                self.error = Some(error.to_string());
            }
        }
    }
    fn advance_wizard(&mut self) {
        let next = match self.wizard {
            Some(WizardStep::Key) => {
                if self.secrets.qwen.trim().is_empty() {
                    self.error = Some("Введите ключ Qwen, чтобы продолжить.".into());
                    return;
                }
                self.secrets.qwen = self.secrets.qwen.trim().to_string();
                WizardStep::Workspace
            }
            Some(WizardStep::Workspace) => {
                if self.settings.workspace.trim().is_empty() {
                    self.error = Some("Укажите рабочую папку.".into());
                    return;
                }
                self.project = self.settings.workspace.clone();
                WizardStep::Install
            }
            _ => return,
        };
        match config::save(&self.settings, &self.secrets) {
            Ok(()) => {
                self.wizard = Some(next);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    fn wizard_ui(&mut self, ui: &mut egui::Ui) {
        let Some(step) = self.wizard else {
            return;
        };
        ui.add_space(20.);
        ui.label(
            RichText::new("Добро пожаловать в Supa Dev")
                .size(30.)
                .strong(),
        );
        ui.label(RichText::new("Подготовим Qwen к работе за несколько шагов.").color(MUTED));
        ui.add_space(20.);
        ui.horizontal(|ui| {
            for (index, label) in ["1  Ключ Qwen", "2  Рабочая папка", "3  Окружение"]
                .iter()
                .enumerate()
            {
                let current = match step {
                    WizardStep::Key => 0,
                    WizardStep::Workspace => 1,
                    _ => 2,
                };
                ui.label(RichText::new(*label).color(if index == current {
                    ACCENT
                } else {
                    MUTED
                }));
                if index < 2 {
                    ui.label(RichText::new("—").color(MUTED));
                }
            }
        });
        ui.add_space(28.);
        match step {
            WizardStep::Key => {
                ui.heading("Введите ключ Qwen");
                ui.label("QwenCloud · Token Plan API Key");
                ui.add(
                    egui::TextEdit::singleline(&mut self.secrets.qwen)
                        .password(true)
                        .desired_width(560.)
                        .hint_text("Вставьте API-ключ"),
                );
                ui.label(RichText::new("Ключ сохраняется только на этом компьютере. Его можно изменить в настройках.").color(MUTED).size(14.));
                if ui
                    .small_button("Использовать ключ из установленного Qwen")
                    .clicked()
                {
                    match config::import_qwen_key() {
                        Some(key) => {
                            self.secrets.qwen = key;
                            self.error = None;
                        }
                        None => self.error = Some("Сохранённый ключ Qwen не найден.".into()),
                    }
                }
                ui.add_space(20.);
                if ui
                    .add_enabled(
                        !self.secrets.qwen.trim().is_empty(),
                        egui::Button::new(RichText::new("Продолжить").color(Color32::BLACK))
                            .fill(ACCENT),
                    )
                    .clicked()
                {
                    self.advance_wizard();
                }
            }
            WizardStep::Workspace => {
                ui.heading("Где будем работать?");
                ui.label("Папка для проектов, временных файлов и кэша");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.workspace).desired_width(620.),
                );
                ui.add_space(8.);
                ui.label("Облачная модель");
                Self::model_select(ui, &mut self.settings.cloud_model);
                ui.label(
                    RichText::new("Для каждого запуска можно выбрать отдельную папку проекта.")
                        .color(MUTED)
                        .size(14.),
                );
                ui.add_space(20.);
                ui.horizontal(|ui| {
                    if ui.button("Назад").clicked() {
                        self.wizard = Some(WizardStep::Key);
                        self.error = None;
                    }
                    if ui
                        .add_enabled(
                            !self.settings.workspace.trim().is_empty(),
                            egui::Button::new(RichText::new("Продолжить").color(Color32::BLACK))
                                .fill(ACCENT),
                        )
                        .clicked()
                    {
                        self.advance_wizard();
                    }
                });
            }
            WizardStep::Install => {
                ui.heading("Подготовка окружения");
                ui.label("Установим Qwen Code, LM Studio и локальный редактор промптов.");
                ui.label(RichText::new("Уже установленные компоненты будут использованы повторно. Для новой модели потребуется скачать 16,8 ГБ; на время установки нужно до 34 ГБ свободного места.").color(MUTED).size(14.));
                ui.add_space(16.);
                if self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&self.progress.0);
                    });
                    ui.add(
                        egui::ProgressBar::new(self.progress.1.clamp(0., 1.))
                            .fill(ACCENT)
                            .animate(true),
                    );
                    if ui.button("Отменить установку").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("Назад").clicked() {
                            self.wizard = Some(WizardStep::Workspace);
                            self.error = None;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Установить окружение").color(Color32::BLACK),
                                )
                                .fill(ACCENT),
                            )
                            .clicked()
                        {
                            self.start(jobs::Action::Install);
                        }
                    });
                    if ui
                        .small_button("Установить позже — открыть приложение")
                        .clicked()
                    {
                        self.finish_setup();
                    }
                }
            }
            WizardStep::Ready => {
                ui.heading("Всё готово к работе");
                ui.label("Откройте папку проекта и запускайте Qwen кнопкой в приложении.");
                ui.add_space(20.);
                if ui
                    .add(
                        egui::Button::new(RichText::new("Начать работу").color(Color32::BLACK))
                            .fill(ACCENT),
                    )
                    .clicked()
                {
                    self.finish_setup();
                }
            }
        }
    }
    fn check_updates(&mut self) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        self.update_message = "Проверка обновлений…".into();
        self.next_update = Instant::now() + Duration::from_secs(6 * 3600);
        jobs::start(
            jobs::Action::CheckUpdate,
            self.settings.clone(),
            self.secrets.clone(),
            self.update_tx.clone(),
            Arc::new(AtomicBool::new(false)),
        );
    }
    fn update_event(&mut self, event: jobs::Event) {
        match event {
            jobs::Event::Update(value) => {
                self.update_prompt = value.path.is_some()
                    && self.postponed_update.as_deref() != Some(value.version.as_str());
                self.update_message = "Обновление скачано и готово к установке".into();
                self.update = Some(value);
            }
            jobs::Event::Progress(text, _) | jobs::Event::UpdateStatus(text) => {
                self.update_message = text
            }
            jobs::Event::Error(error) => {
                self.update_message = error.clone();
                self.log(error);
            }
            jobs::Event::Done(success) => {
                self.update_checking = false;
                if !success {
                    self.next_update = Instant::now() + Duration::from_secs(30 * 60);
                }
            }
            _ => {}
        }
    }
    fn postpone_update(&mut self) {
        self.update_prompt = false;
        self.postponed_update = self.update.as_ref().map(|u| u.version.clone());
    }
    fn operation_finished(&mut self, success: bool) {
        self.busy = false;
        if success {
            self.progress = ("Готово".into(), 1.);
            if matches!(self.active_action, Some(jobs::Action::Install))
                && self.wizard == Some(WizardStep::Install)
            {
                self.wizard = Some(WizardStep::Ready);
            }
        }
        self.active_action = None;
    }
    fn update_modal(&mut self, ctx: &egui::Context) {
        if !self.update_prompt || self.busy || self.wizard.is_some() {
            return;
        }
        let Some(update) = self.update.clone() else {
            return;
        };
        let Some(path) = update.path else {
            return;
        };
        let response = egui::Modal::new(egui::Id::new("update_confirmation"))
            .frame(
                egui::Frame::popup(&ctx.style())
                    .inner_margin(28)
                    .corner_radius(14),
            )
            .show(ctx, |ui| {
                ui.set_width(430.);
                ui.label(
                    RichText::new("Найдено обновление. Обновиться?")
                        .size(22.)
                        .strong(),
                );
                ui.label(format!(
                    "Версия {} уже скачана. Приложение перезапустится.",
                    update.version
                ));
                ui.add_space(12.);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Обновиться").color(Color32::BLACK))
                                .fill(ACCENT),
                        )
                        .clicked()
                    {
                        match updater::restart(&path) {
                            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => {
                                self.update_prompt = false;
                                self.error = Some(e.to_string());
                            }
                        }
                    }
                    if ui.button("Позже").clicked() {
                        self.postpone_update();
                    }
                });
            });
        if response.should_close() {
            self.postpone_update();
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
                jobs::Event::Done(success) => self.operation_finished(success),
                jobs::Event::Update(value) => {
                    self.update_message = "Обновление скачано и проверено по SHA-256".into();
                    self.update = Some(value);
                }
                jobs::Event::UpdateStatus(value) => self.update_message = value,
            }
        }
        while let Ok(event) = self.update_rx.try_recv() {
            self.update_event(event);
        }
        if self.wizard.is_none() && !self.update_checking && Instant::now() >= self.next_update {
            self.check_updates();
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
        if self.wizard.is_none() {
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
        }
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
                    if self.wizard.is_some() {
                        self.wizard_ui(ui);
                    } else {
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
                    }
                });
            });
        self.update_modal(ctx);
        ctx.request_repaint_after(Duration::from_millis(
            if self.busy || self.update_checking {
                100
            } else {
                1000
            },
        ));
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
        Some("--check-updates" | "--download-update") => {
            let (tx, rx) = mpsc::channel();
            let output = std::thread::spawn(move || {
                for event in rx {
                    println!("{event:?}");
                }
            });
            let result = if args[1] == "--download-update" {
                updater::download_latest(&tx, &AtomicBool::new(false))
            } else {
                updater::check(&tx, &AtomicBool::new(false))
            };
            drop(tx);
            let _ = output.join();
            return result;
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

#[cfg(test)]
mod ui_tests {
    use super::*;
    fn app() -> App {
        App::from_settings(config::Settings::default(), config::Secrets::default())
    }
    #[test]
    fn first_launch_and_missing_key_show_wizard_but_completed_setup_does_not() {
        assert_eq!(app().wizard, Some(WizardStep::Key));
        let settings = config::Settings {
            setup_completed: true,
            ..Default::default()
        };
        assert_eq!(
            App::from_settings(settings.clone(), config::Secrets::default()).wizard,
            Some(WizardStep::Key)
        );
        assert!(
            App::from_settings(
                settings,
                config::Secrets {
                    qwen: "synthetic-only".into()
                }
            )
            .wizard
            .is_none()
        );
        let legacy: config::Settings =
            serde_json::from_str(r#"{"auto_updates":false,"workspace":"workspace"}"#).unwrap();
        assert_eq!(
            App::from_settings(
                legacy,
                config::Secrets {
                    qwen: "synthetic-only".into()
                }
            )
            .wizard,
            Some(WizardStep::Key)
        );
    }
    #[test]
    fn failed_or_cancelled_installation_never_completes_wizard() {
        let mut app = app();
        app.wizard = Some(WizardStep::Install);
        app.busy = true;
        app.active_action = Some(jobs::Action::Install);
        app.operation_finished(false);
        assert_eq!(app.wizard, Some(WizardStep::Install));
        assert!(!app.busy);
        app.active_action = Some(jobs::Action::Inspect);
        app.operation_finished(true);
        assert_eq!(app.wizard, Some(WizardStep::Install));
        app.active_action = Some(jobs::Action::Install);
        app.operation_finished(true);
        assert_eq!(app.wizard, Some(WizardStep::Ready));
    }
    #[test]
    fn update_download_does_not_complete_or_block_environment_work() {
        let mut app = app();
        app.busy = true;
        app.active_action = Some(jobs::Action::Install);
        app.progress = ("installation".into(), 0.5);
        app.update_checking = true;
        app.update_event(jobs::Event::Progress("download".into(), 0.1));
        app.update_event(jobs::Event::Done(true));
        assert!(app.busy);
        assert_eq!(app.progress.0, "installation");
        assert!(!app.update_checking);
    }
    #[test]
    fn update_prompt_waits_for_download_and_later_suppresses_same_version() {
        let mut app = app();
        let pending = updater::Update {
            version: "9.0.0".into(),
            path: None,
        };
        app.update_event(jobs::Event::Update(pending));
        assert!(!app.update_prompt);
        let ready = updater::Update {
            version: "9.0.0".into(),
            path: Some("synthetic-update".into()),
        };
        app.update_event(jobs::Event::Update(ready.clone()));
        assert!(app.update_prompt);
        app.postpone_update();
        assert!(!app.update_prompt);
        app.update_event(jobs::Event::Update(ready));
        assert!(!app.update_prompt);
        app.update_event(jobs::Event::Update(updater::Update {
            version: "9.0.1".into(),
            path: Some("synthetic-update".into()),
        }));
        assert!(app.update_prompt);
    }
}
