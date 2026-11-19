mod comparison;
mod jobs;
mod language;
mod theme;

use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::mpsc::{Receiver, TryRecvError},
    time::Duration,
};

use eframe::egui::{
    self, Button, ComboBox, Context, Grid, RichText, ScrollArea, ThemePreference, Ui,
};
use jobs::{Conversion, Event, Failure};
use language::{Catalog, LANGUAGES};
use tenka_core::{OptimizationMode, Options, OutputFormat};

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../packaging/io.github.yrmuq.tenka.png"
    ))
    .expect("the embedded Tenka icon is valid PNG");
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--compare") {
        let language = args.next().unwrap_or_else(|| "system".into());
        let theme = parse_theme(args.next().as_deref());
        return comparison::run(icon, &language, theme);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("io.github.yrmuq.tenka")
            .with_icon(icon)
            .with_inner_size([960.0, 650.0])
            .with_min_inner_size([700.0, 490.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Tenka",
        options,
        Box::new(|context| Ok(Box::new(Tenka::new(context)))),
    )
}

enum Status {
    Pending,
    Running,
    Done(Result<Option<Conversion>, Failure>),
}

struct QueueEntry {
    path: PathBuf,
    status: Status,
}

struct Tenka {
    language: String,
    theme: ThemePreference,
    system_theme: theme::SystemTheme,
    catalog: Catalog,
    queue: Vec<QueueEntry>,
    folder: Option<PathBuf>,
    conversion: Options,
    mode: OptimizationMode,
    folder_failure: Option<Failure>,
    worker: Option<Receiver<Event>>,
    comparison_windows: Vec<Child>,
    comparison_failure: Option<Failure>,
}

impl Tenka {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        let read = |key| context.storage.and_then(|storage| storage.get_string(key));
        let language = read("language")
            .filter(|value| LANGUAGES.contains(&value.as_str()))
            .unwrap_or_else(|| "system".into());
        let theme = parse_theme(read("theme").as_deref());
        context.egui_ctx.set_theme(theme);
        for appearance in [egui::Theme::Light, egui::Theme::Dark] {
            context.egui_ctx.style_mut_of(appearance, |style| {
                style.spacing.item_spacing = egui::vec2(10.0, 8.0);
                style.spacing.button_padding = egui::vec2(8.0, 3.0);
                style
                    .text_styles
                    .get_mut(&egui::TextStyle::Body)
                    .unwrap()
                    .size = 14.0;
                style
                    .text_styles
                    .get_mut(&egui::TextStyle::Button)
                    .unwrap()
                    .size = 14.0;
                style.visuals.selection.bg_fill = if appearance == egui::Theme::Light {
                    egui::Color32::from_rgb(209, 228, 251)
                } else {
                    style.visuals.override_text_color = Some(egui::Color32::from_gray(213));
                    egui::Color32::from_rgb(31, 68, 108)
                };
            });
        }
        Self {
            catalog: Catalog::new(&language),
            language,
            theme,
            system_theme: theme::SystemTheme::new(&context.egui_ctx),
            queue: Vec::new(),
            folder: None,
            conversion: Options::default(),
            mode: OptimizationMode::default(),
            folder_failure: None,
            worker: None,
            comparison_windows: Vec::new(),
            comparison_failure: None,
        }
    }

    fn label(&self, id: u16) -> &'static str {
        self.catalog.text(id)
    }

    fn header(&mut self, context: &Context) {
        egui::TopBottomPanel::top("navigation")
            .frame(
                egui::Frame::side_top_panel(&context.style())
                    .inner_margin(egui::Margin::symmetric(20, 11)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("Tenka").strong().size(24.0));
                    ui.add_space(12.0);
                    ui.label(RichText::new(self.label(1)).strong());
                    if ui.button(self.label(109)).clicked() {
                        self.open_comparison();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let theme_label =
                            format!("{} · {}", self.label(4), self.label(theme_id(self.theme)));
                        let options = [
                            ThemePreference::System,
                            ThemePreference::Light,
                            ThemePreference::Dark,
                        ]
                        .map(|theme| (theme, self.label(theme_id(theme))));
                        let previous_theme = self.theme;
                        ComboBox::from_id_salt("theme")
                            .selected_text(theme_label)
                            .width(126.0)
                            .show_ui(ui, |ui| {
                                for (theme, label) in options {
                                    ui.selectable_value(&mut self.theme, theme, label);
                                }
                            })
                            .response
                            .on_hover_text(self.label(4));
                        if self.theme != previous_theme {
                            self.system_theme.apply(context, self.theme);
                        }

                        let system = self.label(5);
                        let language_label = |code: &str| match code {
                            "en" => "English",
                            "ru" => "Русский",
                            _ => system,
                        };
                        let previous_language = self.language.clone();
                        ComboBox::from_id_salt("language")
                            .selected_text(format!(
                                "{} · {}",
                                self.label(3),
                                language_label(&self.language)
                            ))
                            .width(126.0)
                            .show_ui(ui, |ui| {
                                for code in LANGUAGES {
                                    ui.selectable_value(
                                        &mut self.language,
                                        code.into(),
                                        language_label(code),
                                    );
                                }
                            })
                            .response
                            .on_hover_text(self.label(3));
                        if self.language != previous_language {
                            self.catalog = Catalog::new(&self.language);
                        }
                    });
                });
            });
    }

    fn show_convert(&mut self, ui: &mut Ui) {
        ui.heading(self.label(72));
        let busy = self.worker.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, Button::new(self.label(8))).clicked()
                && let Some(files) = self.picker().pick_files()
            {
                self.add_files(files);
            }
            if ui
                .add_enabled(!busy && !self.queue.is_empty(), Button::new(self.label(23)))
                .clicked()
            {
                self.queue.clear();
            }
        });
        folder_picker(ui, &self.catalog, &mut self.folder, busy);

        ui.add_space(6.0);
        section(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                format_picker(ui, self.label(73), &mut self.conversion.format);
                ui.horizontal(|ui| {
                    ui.label(self.label(110));
                    for (mode, label, hint) in [
                        (OptimizationMode::Best, 111, 114),
                        (OptimizationMode::Balance, 112, 115),
                        (OptimizationMode::Cheap, 113, 116),
                    ] {
                        let label = self.label(label);
                        let hint = self.label(hint);
                        ui.selectable_value(&mut self.mode, mode, label)
                            .on_hover_text(hint);
                    }
                });
                let resize = self.label(74);
                ui.checkbox(&mut self.conversion.auto_resize, resize)
                    .on_hover_text(self.label(75));
            });
        });

        let pending = self
            .queue
            .iter()
            .filter(|entry| matches!(entry.status, Status::Pending))
            .count();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy && pending > 0,
                    Button::new(self.label(22)).min_size([140.0, 27.0].into()),
                )
                .clicked()
            {
                self.folder_failure = None;
                let entries = self
                    .queue
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        matches!(entry.status, Status::Pending)
                            .then_some((index, entry.path.clone()))
                    })
                    .collect();
                self.worker = Some(jobs::convert(
                    ui.ctx(),
                    entries,
                    self.folder.clone(),
                    self.conversion,
                    self.mode,
                ));
            }
            if pending > 0 {
                ui.weak(format!("{}  {pending}", self.label(68)));
            }
            if busy
                && self
                    .queue
                    .iter()
                    .any(|e| matches!(e.status, Status::Running))
            {
                ui.spinner();
            }
        });
        if let Some(error) = &self.folder_failure {
            self.show_error(ui, error);
        }
        if let Some(error) = &self.comparison_failure {
            self.show_error(ui, error);
        }
        ui.separator();

        if self.queue.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(self.label(11)).size(18.0));
            });
            return;
        }
        let mut folder_failure = None;
        ScrollArea::vertical().show(ui, |ui| {
            Grid::new("files")
                .num_columns(3)
                .striped(true)
                .min_col_width(145.0)
                .show(ui, |ui| {
                    for id in [28, 76, 30] {
                        ui.strong(self.label(id));
                    }
                    ui.end_row();
                    for entry in &self.queue {
                        ui.add(egui::Label::new(file_name(&entry.path)).truncate())
                            .on_hover_text(path_label(&entry.path));
                        match &entry.status {
                            Status::Done(Ok(Some(result))) => {
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.weak(size(result.source_bytes));
                                        let (arrow, _) = ui.allocate_exact_size(
                                            egui::vec2(12.0, 10.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().arrow(
                                            arrow.left_center(),
                                            egui::vec2(arrow.width(), 0.0),
                                            egui::Stroke::new(
                                                1.0_f32,
                                                ui.visuals().weak_text_color(),
                                            ),
                                        );
                                        ui.strong(format!(
                                            "{} · {} × {}",
                                            size(result.bytes),
                                            result.dimensions.0,
                                            result.dimensions.1
                                        ));
                                        ui.weak(if result.lossless {
                                            self.label(17).into()
                                        } else {
                                            format!("{:.1}", result.quality)
                                        });
                                    });
                                    ui.add(egui::Label::new(file_name(&result.path)).truncate())
                                        .on_hover_text(path_label(&result.path));
                                });
                                ui.vertical(|ui| {
                                    ui.label(self.label(26));
                                    if ui.small_button(self.label(70)).clicked()
                                        && let Some(parent) = result.path.parent()
                                        && let Err(error) = std::process::Command::new("xdg-open")
                                            .arg(parent)
                                            .spawn()
                                    {
                                        folder_failure = Some(Failure {
                                            label: 71,
                                            detail: error.to_string(),
                                        });
                                    }
                                });
                            }
                            other => {
                                ui.weak("—");
                                match other {
                                    Status::Pending => {
                                        ui.weak(self.label(24));
                                    }
                                    Status::Running => {
                                        ui.label(self.label(25));
                                    }
                                    Status::Done(Err(error)) => self.show_error(ui, error),
                                    Status::Done(Ok(None)) => {
                                        ui.weak(self.label(107));
                                    }
                                    Status::Done(Ok(Some(_))) => unreachable!(),
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
        });
        if folder_failure.is_some() {
            self.folder_failure = folder_failure;
        }
    }

    fn picker(&self) -> rfd::FileDialog {
        rfd::FileDialog::new().add_filter(self.label(65), &["jpg", "jpeg", "png", "webp"])
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if !self.queue.iter().any(|existing| existing.path == path) {
                self.queue.push(QueueEntry {
                    path,
                    status: Status::Pending,
                });
            }
        }
    }

    fn poll_worker(&mut self) {
        while let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(Event::Started(index)) => self.queue[index].status = Status::Running,
                Ok(Event::Converted(index, result)) => {
                    self.queue[index].status = Status::Done(result)
                }
                Ok(Event::Finished) => self.worker = None,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let mut conversion = false;
                    for entry in &mut self.queue {
                        if matches!(entry.status, Status::Running) {
                            entry.status = Status::Done(Err(Failure {
                                label: 61,
                                detail: String::new(),
                            }));
                            conversion = true;
                        }
                    }
                    if !conversion {
                        self.folder_failure = Some(Failure {
                            label: 61,
                            detail: String::new(),
                        });
                    }
                    self.worker = None;
                }
            }
        }
    }

    fn open_comparison(&mut self) {
        self.comparison_failure = None;
        let child = std::env::current_exe().and_then(|executable| {
            let mut command = Command::new(executable);
            command.args(["--compare", &self.language, theme_name(self.theme)]);
            // Winit only delivers file drops on X11. Use XWayland for this window when available.
            if std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
                command
                    .env_remove("WAYLAND_DISPLAY")
                    .env_remove("WAYLAND_SOCKET");
            }
            command.spawn()
        });
        match child {
            Ok(child) => self.comparison_windows.push(child),
            Err(error) => {
                self.comparison_failure = Some(Failure {
                    label: 101,
                    detail: error.to_string(),
                });
            }
        }
    }

    fn poll_comparison_windows(&mut self, context: &Context) {
        self.comparison_windows.retain_mut(|child| {
            let (label, detail) = match child.try_wait() {
                Ok(None) => return true,
                Ok(Some(status)) if status.success() => return false,
                Ok(Some(status)) => (102, status.to_string()),
                Err(error) => (103, error.to_string()),
            };
            self.comparison_failure = Some(Failure { label, detail });
            false
        });
        if !self.comparison_windows.is_empty() {
            context.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn show_error(&self, ui: &mut Ui, error: &Failure) {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("[E{}] {}", error.label, self.label(error.label)),
        )
        .on_hover_text(&error.detail);
    }
}

impl eframe::App for Tenka {
    fn update(&mut self, context: &Context, _: &mut eframe::Frame) {
        self.system_theme.apply(context, self.theme);
        self.poll_worker();
        if self.worker.is_none() {
            let files = context.input(|input| {
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect::<Vec<_>>()
            });
            self.add_files(files);
        }
        self.header(context);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&context.style())
                    .inner_margin(egui::Margin::symmetric(20, 15)),
            )
            .show(context, |ui| self.show_convert(ui));
        self.poll_comparison_windows(context);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("language", self.language.clone());
        storage.set_string("theme", theme_name(self.theme).into());
    }
}

fn parse_theme(name: Option<&str>) -> ThemePreference {
    match name {
        Some("light") => ThemePreference::Light,
        Some("dark") => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

fn theme_name(theme: ThemePreference) -> &'static str {
    match theme {
        ThemePreference::System => "system",
        ThemePreference::Light => "light",
        ThemePreference::Dark => "dark",
    }
}

fn theme_id(theme: ThemePreference) -> u16 {
    match theme {
        ThemePreference::System => 5,
        ThemePreference::Light => 6,
        ThemePreference::Dark => 7,
    }
}

fn folder_picker(ui: &mut Ui, catalog: &Catalog, folder: &mut Option<PathBuf>, busy: bool) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, Button::new(catalog.text(9)))
            .on_hover_text(catalog.text(63))
            .clicked()
            && let Some(selected) = rfd::FileDialog::new().pick_folder()
        {
            *folder = Some(selected);
        }
        if let Some(selected) = folder {
            if ui
                .add_enabled(!busy, Button::new(catalog.text(10)))
                .clicked()
            {
                *folder = None;
            } else {
                let path = path_label(selected);
                ui.add(egui::Label::new(&path).truncate())
                    .on_hover_text(path);
            }
        } else {
            ui.weak(catalog.text(10));
        }
    });
}

fn format_picker(ui: &mut Ui, label: &str, format: &mut OutputFormat) {
    ui.horizontal(|ui| {
        ui.label(label);
        ComboBox::from_id_salt("output_format")
            .selected_text(format.name())
            .width(164.0)
            .show_ui(ui, |ui| {
                for option in OutputFormat::ALL {
                    ui.selectable_value(format, option, option.name());
                }
            });
    });
}

fn section(ui: &mut Ui, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
}

fn path_label(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}
