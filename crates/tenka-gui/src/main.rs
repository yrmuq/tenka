mod comparison;
mod file_drop;
mod icons;
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
use file_drop::FileDrops;
use jobs::{Conversion, Event, Failure};
use language::Catalog;
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
    file_drops: FileDrops,
    catalog: Catalog,
    queue: Vec<QueueEntry>,
    folder: Option<PathBuf>,
    format: OutputFormat,
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
            .filter(|value| value == "en")
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
            file_drops: FileDrops::new(&context.egui_ctx),
            queue: Vec::new(),
            folder: None,
            format: OutputFormat::default(),
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
                    if ui.button(self.label(109)).clicked() {
                        self.open_comparison();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let theme_label =
                            format!("{} · {}", self.label(4), self.label(theme_id(self.theme)));
                        if icons::theme(ui, self.theme, &theme_label).clicked() {
                            self.theme = match self.theme {
                                ThemePreference::System => ThemePreference::Light,
                                ThemePreference::Light => ThemePreference::Dark,
                                ThemePreference::Dark => ThemePreference::System,
                            };
                            self.system_theme.apply(context, self.theme);
                        }

                        let language = if self.language == "en" {
                            "English"
                        } else {
                            self.label(5)
                        };
                        let language_label = format!("{} · {language}", self.label(3));
                        if icons::globe(ui, &language_label).clicked() {
                            self.language = if self.language == "en" {
                                "system"
                            } else {
                                "en"
                            }
                            .into();
                            self.catalog = Catalog::new(&self.language);
                        }
                        ui.add_enabled_ui(self.worker.is_none(), |ui| {
                            self.conversion_settings(ui);
                        });
                    });
                });
            });
    }

    fn conversion_settings(&mut self, ui: &mut Ui) {
        ComboBox::from_id_salt("output_format")
            .selected_text(self.format.name())
            .width(96.0)
            .show_ui(ui, |ui| {
                for format in OutputFormat::ALL {
                    ui.selectable_value(&mut self.format, format, format.name());
                }
            })
            .response
            .on_hover_text(self.label(73));

        let modes = [
            (OptimizationMode::Best, 111, 114),
            (OptimizationMode::Balance, 112, 115),
            (OptimizationMode::Cheap, 113, 116),
        ];
        let selected = match self.mode {
            OptimizationMode::Best => 111,
            OptimizationMode::Balance => 112,
            OptimizationMode::Cheap => 113,
        };
        ComboBox::from_id_salt("mode")
            .selected_text(self.label(selected))
            .width(96.0)
            .show_ui(ui, |ui| {
                for (mode, label, hint) in modes {
                    let label = self.label(label);
                    let hint = self.label(hint);
                    ui.selectable_value(&mut self.mode, mode, label)
                        .on_hover_text(hint);
                }
            })
            .response
            .on_hover_text(self.label(110));
    }

    fn show_convert(&mut self, ui: &mut Ui) {
        let busy = self.worker.is_some();
        let pending = self
            .queue
            .iter()
            .filter(|entry| matches!(entry.status, Status::Pending))
            .count();
        let action_size = egui::vec2(140.0, 27.0);
        let (actions, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), action_size.y),
            egui::Sense::hover(),
        );
        let center = egui::Rect::from_center_size(actions.center(), action_size);
        let offset = egui::vec2(action_size.x + ui.spacing().item_spacing.x, 0.0);
        let destination = self
            .folder
            .as_deref()
            .map(path_label)
            .unwrap_or_else(|| self.label(10).into());
        if ui
            .put(center.translate(-offset), |ui: &mut Ui| {
                ui.add_enabled(!busy, Button::new(self.label(9)))
            })
            .on_hover_text(destination)
            .clicked()
            && let Some(selected) = rfd::FileDialog::new().pick_folder()
        {
            self.folder = Some(selected);
        }
        if ui
            .put(center, |ui: &mut Ui| {
                ui.add_enabled(!busy && pending > 0, Button::new(self.label(22)))
            })
            .clicked()
        {
            self.folder_failure = None;
            let entries = self
                .queue
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    matches!(entry.status, Status::Pending).then_some((index, entry.path.clone()))
                })
                .collect();
            self.worker = Some(jobs::convert(
                ui.ctx(),
                entries,
                self.folder.clone(),
                Options {
                    format: self.format,
                    auto_resize: false,
                },
                self.mode,
            ));
        }
        if ui
            .put(center.translate(offset), |ui: &mut Ui| {
                ui.add_enabled(!busy && !self.queue.is_empty(), Button::new(self.label(23)))
            })
            .clicked()
        {
            self.queue.clear();
        }
        if self.folder.is_some() || busy || pending > 0 {
            let (details, _) =
                ui.allocate_exact_size(egui::vec2(actions.width(), 20.0), egui::Sense::hover());
            let center =
                egui::Rect::from_center_size(details.center(), egui::vec2(action_size.x, 20.0));
            if self.folder.is_some()
                && ui
                    .put(center.translate(-offset), |ui: &mut Ui| {
                        ui.add_enabled(!busy, Button::new(self.label(10)))
                    })
                    .clicked()
            {
                self.folder = None;
            }
            if busy {
                ui.put(center, egui::Spinner::new());
            } else if pending > 0 {
                ui.put(
                    center,
                    egui::Label::new(
                        RichText::new(format!("{}  {pending}", self.label(68))).weak(),
                    ),
                );
            }
        }
        if let Some(error) = &self.folder_failure {
            self.show_error(ui, error);
        }
        if let Some(error) = &self.comparison_failure {
            self.show_error(ui, error);
        }
        ui.separator();

        if add_images_prompt(ui, &self.catalog, !busy)
            && let Some(files) = self.picker().pick_files()
        {
            self.add_files(files);
        }
        if self.queue.is_empty() {
            return;
        }
        let mut folder_failure = None;
        ScrollArea::vertical().show(ui, |ui| {
            Grid::new("files")
                .num_columns(2)
                .striped(true)
                .min_col_width(145.0)
                .show(ui, |ui| {
                    for id in [28, 76] {
                        ui.strong(self.label(id));
                    }
                    ui.end_row();
                    for entry in &self.queue {
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(file_name(&entry.path)).truncate())
                                .on_hover_text(path_label(&entry.path));
                            if let Status::Done(Err(error)) = &entry.status {
                                self.show_error(ui, error);
                            }
                        });
                        match &entry.status {
                            Status::Done(Ok(Some(result))) => {
                                ui.add(
                                    egui::Label::new(format!(
                                        "{} -> {}",
                                        self.catalog.file_size(result.source_bytes),
                                        self.catalog.file_size(result.bytes),
                                    ))
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(path_label(&result.path))
                                .context_menu(|ui| {
                                    if ui.button(self.label(70)).clicked() {
                                        if let Some(parent) = result.path.parent()
                                            && let Err(error) =
                                                Command::new("xdg-open").arg(parent).spawn()
                                        {
                                            folder_failure = Some(Failure {
                                                label: 71,
                                                detail: error.to_string(),
                                            });
                                        }
                                        ui.close();
                                    }
                                });
                            }
                            Status::Done(Ok(None)) => {
                                ui.weak("—").on_hover_text(self.label(107));
                            }
                            _ => {
                                ui.weak("—");
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

    fn add_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
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
            Command::new(executable)
                .args(["--compare", &self.language, theme_name(self.theme)])
                .spawn()
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
        self.add_files(self.file_drops.take(context));
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

fn add_images_prompt(ui: &mut Ui, catalog: &Catalog, enabled: bool) -> bool {
    ui.vertical_centered(|ui| {
        let width = ui.available_width().min(240.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                egui::Frame::group(ui.style())
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(catalog.text(11));
                        let response = ui.add_enabled(
                            enabled,
                            Button::new(RichText::new("+").size(20.0))
                                .min_size(egui::vec2(34.0, 30.0)),
                        );
                        let label = catalog.text(8);
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label)
                        });
                        response.on_hover_text(label).clicked()
                    })
                    .inner
            },
        )
        .inner
    })
    .inner
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
