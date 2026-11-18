mod jobs;
mod language;

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError},
};

use eframe::egui::{
    self, Button, ColorImage, ComboBox, Context, Grid, RichText, ScrollArea, TextureHandle,
    ThemePreference, Ui, Vec2,
};
use jobs::{Comparison, Conversion, Event, Failure};
use language::{Catalog, LANGUAGES};
use tenka_core::{DEFAULT_QUALITY, Options, OutputFormat, Setting};

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../packaging/io.github.yrmuq.tenka.png"
    ))
    .expect("the embedded Tenka icon is valid PNG");
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

#[derive(PartialEq)]
enum Tab {
    Convert,
    Tests,
}

enum Status {
    Pending,
    Running,
    Done(Result<Conversion, Failure>),
}

struct QueueEntry {
    path: PathBuf,
    status: Status,
}

struct TestRow {
    format: OutputFormat,
    outcome: Result<Comparison, Failure>,
}

struct Tenka {
    tab: Tab,
    language: String,
    theme: ThemePreference,
    catalog: Catalog,
    queue: Vec<QueueEntry>,
    folder: Option<PathBuf>,
    conversion: Options,
    test_file: Option<PathBuf>,
    test_quality: u8,
    test_auto_resize: bool,
    test_rows: Vec<TestRow>,
    original: Option<(TextureHandle, String)>,
    preview: Option<(usize, TextureHandle)>,
    test_failure: Option<Failure>,
    folder_failure: Option<Failure>,
    worker: Option<Receiver<Event>>,
}

impl Tenka {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        let read = |key| context.storage.and_then(|storage| storage.get_string(key));
        let language = read("language")
            .filter(|value| LANGUAGES.contains(&value.as_str()))
            .unwrap_or_else(|| "system".into());
        let theme = match read("theme").as_deref() {
            Some("light") => ThemePreference::Light,
            Some("dark") => ThemePreference::Dark,
            _ => ThemePreference::System,
        };
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
            tab: Tab::Convert,
            catalog: Catalog::new(&language),
            language,
            theme,
            queue: Vec::new(),
            folder: None,
            conversion: Options::default(),
            test_file: None,
            test_quality: DEFAULT_QUALITY,
            test_auto_resize: false,
            test_rows: Vec::new(),
            original: None,
            preview: None,
            test_failure: None,
            folder_failure: None,
            worker: None,
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
                    if ui
                        .selectable_label(self.tab == Tab::Convert, self.label(1))
                        .clicked()
                    {
                        self.tab = Tab::Convert;
                    }
                    if ui
                        .selectable_label(self.tab == Tab::Tests, self.label(2))
                        .clicked()
                    {
                        self.tab = Tab::Tests;
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
                            context.set_theme(self.theme);
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
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, Button::new(self.label(9)))
                .on_hover_text(self.label(63))
                .clicked()
                && let Some(folder) = rfd::FileDialog::new().pick_folder()
            {
                self.folder = Some(folder);
            }
            if let Some(folder) = &self.folder {
                ui.label(path_label(folder));
                if ui.add_enabled(!busy, Button::new(self.label(10))).clicked() {
                    self.folder = None;
                }
            } else {
                ui.weak(self.label(10));
            }
        });

        ui.add_space(6.0);
        section(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    ui.label(self.label(73));
                    ComboBox::from_id_salt("output_format")
                        .selected_text(self.conversion.format.name())
                        .width(164.0)
                        .show_ui(ui, |ui| {
                            for format in OutputFormat::ALL {
                                ui.selectable_value(
                                    &mut self.conversion.format,
                                    format,
                                    format.name(),
                                );
                            }
                        });
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
                ));
            }
            if pending > 0 {
                ui.weak(format!("{}  {pending}", self.label(68)));
            }
            if matches!(self.tab, Tab::Convert)
                && busy
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
                            Status::Done(Ok(result)) => {
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
                                    Status::Done(Ok(_)) => unreachable!(),
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

    fn show_tests(&mut self, ui: &mut Ui) {
        ui.heading(self.label(34));
        let busy = self.worker.is_some();
        section(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                let quality = ui
                    .horizontal(|ui| {
                        ui.label(self.label(82));
                        ui.add(egui::DragValue::new(&mut self.test_quality).range(1_u8..=100))
                            .on_hover_text(self.label(87))
                            .changed()
                    })
                    .inner;
                let resize = self.label(74);
                let resize = ui.checkbox(&mut self.test_auto_resize, resize).changed();
                if quality || resize {
                    self.clear_test_results();
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, Button::new(self.label(32))).clicked()
                && let Some(file) = self.picker().pick_file()
            {
                self.set_test_file(file);
            }
            if ui
                .add_enabled(
                    !busy && self.test_file.is_some(),
                    Button::new(self.label(33)),
                )
                .clicked()
                && let Some(file) = self.test_file.clone()
            {
                self.clear_test_results();
                self.worker = Some(jobs::compare(
                    ui.ctx(),
                    file,
                    self.test_quality,
                    self.test_auto_resize,
                ));
            }
            if busy
                && !self
                    .queue
                    .iter()
                    .any(|e| matches!(e.status, Status::Running))
            {
                ui.spinner();
                ui.label(self.label(62));
            }
        });
        if let Some(file) = &self.test_file {
            ui.strong(file_name(file)).on_hover_text(path_label(file));
        } else {
            ui.weak(self.label(35));
        }
        if let Some(error) = &self.test_failure {
            self.show_error(ui, error);
        }
        if self.test_rows.is_empty() {
            return;
        }
        ui.separator();

        ScrollArea::vertical().show(ui, |ui| {
            let mut selection = None;
            Grid::new("measurements")
                .num_columns(6)
                .striped(true)
                .spacing([18.0, 5.0])
                .show(ui, |ui| {
                    for id in [36, 37, 38, 83, 85, 84] {
                        ui.strong(self.label(id));
                    }
                    ui.end_row();
                    for (index, row) in self.test_rows.iter().enumerate() {
                        let codec = row.format.name();
                        let selected = self.preview.as_ref().is_some_and(|(idx, _)| *idx == index);
                        match &row.outcome {
                            Ok(result) => {
                                if ui.selectable_label(selected, codec).clicked() {
                                    selection = Some((index, &result.preview));
                                }
                                let setting = match result.setting {
                                    Setting::Jxl(value) => format!("{} {value}", self.label(64)),
                                    Setting::Webp(value) => format!("{} {value}", self.label(39)),
                                    Setting::JxlLossless | Setting::WebpLossless => {
                                        self.label(17).into()
                                    }
                                };
                                ui.label(setting);
                                ui.label(size(result.bytes));
                                ui.label(dimensions_text(result.dimensions));
                                ui.label(format!("{:.1}%", result.quality));
                                ui.label(format!("{:.0} ms", result.milliseconds));
                            }
                            Err(error) => {
                                ui.label(codec);
                                self.show_error(ui, error);
                                for _ in 0..4 {
                                    ui.weak("—");
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
            if let Some((index, pixels)) = selection {
                self.preview = Some((index, texture(ui.ctx(), "encoded", pixels.clone())));
            }
            ui.weak(self.label(47));
            ui.separator();
            if let (Some((original, dimensions)), Some((index, encoded))) =
                (&self.original, &self.preview)
                && let Ok(result) = &self.test_rows[*index].outcome
            {
                ui.columns(2, |columns| {
                    let original_size = original.size_vec2();
                    let width = columns[0]
                        .available_width()
                        .min(columns[1].available_width());
                    let scale = (width / original_size.x)
                        .min(260.0 / original_size.y)
                        .min(1.0);
                    let preview_size = original_size * scale;
                    show_preview(
                        &mut columns[0],
                        original,
                        self.label(44),
                        dimensions,
                        preview_size,
                    );
                    show_preview(
                        &mut columns[1],
                        encoded,
                        self.label(45),
                        &dimensions_text(result.dimensions),
                        preview_size,
                    );
                });
            } else {
                ui.weak(self.label(46));
            }
        });
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

    fn set_test_file(&mut self, file: PathBuf) {
        self.test_file = Some(file);
        self.clear_test_results();
    }

    fn clear_test_results(&mut self) {
        self.test_rows.clear();
        self.original = None;
        self.preview = None;
        self.test_failure = None;
    }

    fn poll_worker(&mut self, context: &Context) {
        while let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(Event::Started(index)) => self.queue[index].status = Status::Running,
                Ok(Event::Converted(index, result)) => {
                    self.queue[index].status = Status::Done(result)
                }
                Ok(Event::Original(pixels, dimensions)) => {
                    self.original = Some((texture(context, "original", pixels), dimensions));
                }
                Ok(Event::Measured(format, outcome)) => {
                    if self.preview.is_none()
                        && let Ok(result) = &outcome
                    {
                        self.preview = Some((
                            self.test_rows.len(),
                            texture(context, "encoded", result.preview.clone()),
                        ));
                    }
                    self.test_rows.push(TestRow { format, outcome });
                }
                Ok(Event::Failed(error)) => self.test_failure = Some(error),
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
                        self.test_failure = Some(Failure {
                            label: 61,
                            detail: String::new(),
                        });
                    }
                    self.worker = None;
                }
            }
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
        self.poll_worker(context);
        if self.worker.is_none() {
            let files = context.input(|input| {
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect::<Vec<_>>()
            });
            match self.tab {
                Tab::Convert => self.add_files(files),
                Tab::Tests => {
                    if let Some(file) = files.into_iter().next() {
                        self.set_test_file(file);
                    }
                }
            }
        }
        self.header(context);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&context.style())
                    .inner_margin(egui::Margin::symmetric(20, 15)),
            )
            .show(context, |ui| match self.tab {
                Tab::Convert => self.show_convert(ui),
                Tab::Tests => self.show_tests(ui),
            });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("language", self.language.clone());
        let theme = match self.theme {
            ThemePreference::System => "system",
            ThemePreference::Light => "light",
            ThemePreference::Dark => "dark",
        };
        storage.set_string("theme", theme.into());
    }
}

fn theme_id(theme: ThemePreference) -> u16 {
    match theme {
        ThemePreference::System => 5,
        ThemePreference::Light => 6,
        ThemePreference::Dark => 7,
    }
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

fn texture(context: &Context, name: &str, pixels: ColorImage) -> TextureHandle {
    context.load_texture(name, pixels, egui::TextureOptions::LINEAR)
}

fn dimensions_text((width, height): (u32, u32)) -> String {
    format!("{width} × {height}")
}

fn show_preview(ui: &mut Ui, texture: &TextureHandle, label: &str, dimensions: &str, size: Vec2) {
    ui.strong(label);
    ui.weak(dimensions);
    ui.add(egui::Image::new(texture).fit_to_exact_size(size));
}
