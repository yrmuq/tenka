mod about;
mod comparison;
mod estimate;
mod file_drop;
mod icons;
mod image_info;
mod jobs;
mod language;
mod summary;
mod theme;
mod updates;
#[cfg(target_os = "linux")]
mod window_icon;
mod window_size;

use std::{
    collections::HashSet,
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, TryRecvError},
    },
    time::Duration,
};

use eframe::egui::{
    self, Button, ComboBox, Context, Rect, RichText, ScrollArea, ThemePreference, Ui,
    scroll_area::ScrollBarVisibility,
};
use estimate::{Estimate, InputSize};
use file_drop::take_dropped_paths;
use jobs::{Conversion, Event, Failure, InputFile, InputProbe, ScannedInputs};
use language::Catalog;
use summary::Summary;
use tenka_core::{OptimizationMode, Options, OutputFormat};
use window_size::{DEFAULT_SIZE as MAIN_WINDOW_SIZE, ToolsPosition, WindowSize};

const ADD_IMAGES_SIZE: egui::Vec2 = egui::vec2(34.0, 30.0);
const ACTION_GAP: f32 = 15.0;
const DROP_AREA_SIDE_MARGIN: i8 = 20;
const SELECTOR_WIDTH: f32 = 96.0;
const APP_ID: &str = "io.github.yrmuq.tenka";
const APP_ICON: &[u8] = include_bytes!("../../../assets/io.github.yrmuq.tenka.png");

fn app_title(uppercase: bool) -> &'static str {
    if uppercase { "TENKA" } else { "Tenka" }
}

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    window_icon::register_appimage_icon();

    let icon =
        eframe::icon_data::from_png_bytes(APP_ICON).expect("the embedded Tenka icon is valid PNG");
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(OsStr::new("--compare")) {
        let language = args
            .next()
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| "system".into());
        let theme = args.next();
        let theme = parse_theme(theme.as_deref().and_then(OsStr::to_str));
        return comparison::run(icon, &language, theme, args.map(PathBuf::from).collect());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_icon(icon),
        window_builder: Some(Box::new(|window| {
            window
                .with_inner_size(MAIN_WINDOW_SIZE)
                .with_min_inner_size(window_size::MIN_SIZE)
                .with_resizable(true)
                .with_maximized(false)
                .with_fullscreen(false)
        })),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        app_title(false),
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
    input: InputFile,
    size: Arc<OnceLock<InputSize>>,
    status: Status,
}

struct Batch {
    events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    queue_end: usize,
    estimate: Estimate,
}

impl Batch {
    fn estimate_seconds(&self, queue: &[QueueEntry]) -> Option<u64> {
        if self.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        self.estimate.seconds(
            queue[..self.queue_end]
                .iter()
                .filter(|entry| matches!(entry.status, Status::Pending | Status::Running))
                .map(|entry| entry.size.get().copied().unwrap_or_default()),
        )
    }
}

struct Tenka {
    language: String,
    theme: ThemePreference,
    system_theme: theme::SystemTheme,
    catalog: Catalog,
    about: about::About,
    image_info: image_info::ImageInfo,
    window_size: WindowSize,
    queue: Vec<QueueEntry>,
    queue_jump: Option<usize>,
    scan_requests: Sender<Vec<PathBuf>>,
    scanned_inputs: Receiver<ScannedInputs>,
    scans_pending: usize,
    input_probes: Sender<Vec<InputProbe>>,
    file_column_fraction: f32,
    folder: Option<PathBuf>,
    format: OutputFormat,
    mode: OptimizationMode,
    folder_failure: Option<Failure>,
    folder_request: Option<Receiver<Result<(), Failure>>>,
    worker: Option<Batch>,
    batch_summary: Summary,
    completed_summary: Option<Summary>,
    open_batch_errors: bool,
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
        let format = read("output_format");
        let format = OutputFormat::ALL
            .into_iter()
            .find(|item| Some(item.extension()) == format.as_deref())
            .unwrap_or_default();
        let mode = match read("optimization_mode").as_deref() {
            Some("balance") => OptimizationMode::Balance,
            Some("cheap") => OptimizationMode::Cheap,
            _ => OptimizationMode::Best,
        };
        context.egui_ctx.set_theme(theme);
        theme::apply_light_text_contrast(&context.egui_ctx);
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
        let (scan_requests, scanned_inputs) = jobs::input_scanner(&context.egui_ctx);
        Self {
            catalog: Catalog::new(&language),
            about: about::About::default(),
            image_info: image_info::ImageInfo::default(),
            window_size: WindowSize::new(),
            language,
            theme,
            system_theme: theme::SystemTheme::new(&context.egui_ctx),
            queue: Vec::new(),
            queue_jump: None,
            scan_requests,
            scanned_inputs,
            scans_pending: 0,
            input_probes: jobs::probe_inputs(),
            file_column_fraction: 0.5,
            folder: None,
            format,
            mode,
            folder_failure: None,
            folder_request: None,
            worker: None,
            batch_summary: Summary::default(),
            completed_summary: None,
            open_batch_errors: false,
            comparison_windows: Vec::new(),
            comparison_failure: None,
        }
    }

    fn label(&self, id: u16) -> &'static str {
        self.catalog.text(id)
    }

    fn header(&mut self, ui: &mut Ui) {
        let tools_position = self.window_size.tools_position();
        egui::Panel::top("navigation")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(20, 11)),
            )
            .show(ui, |ui| {
                let center_x = ui.available_rect_before_wrap().center().x;
                let tools_size = (tools_position == ToolsPosition::Center).then(|| {
                    let font = egui::TextStyle::Button.resolve(ui.style());
                    let label = ui.painter().layout_no_wrap(
                        self.label(183).into(),
                        font,
                        ui.visuals().text_color(),
                    );
                    let padded = label.size() + 2.0 * ui.spacing().button_padding;
                    padded.max(ui.spacing().interact_size)
                });
                let row = ui.horizontal(|ui| {
                    ui.add_enabled_ui(self.worker.is_none(), |ui| {
                        self.quality_selector(ui);
                        self.format_selector(ui);
                    });
                    if tools_position == ToolsPosition::Left {
                        self.tools_menu(ui);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::about(ui, self.label(153)).clicked() {
                            self.about.open();
                        }
                        let theme_label =
                            format!("{} · {}", self.label(4), self.label(theme_id(self.theme)));
                        if icons::theme(ui, self.theme, &theme_label).clicked() {
                            self.theme = match self.theme {
                                ThemePreference::System => ThemePreference::Light,
                                ThemePreference::Light => ThemePreference::Dark,
                                ThemePreference::Dark => ThemePreference::System,
                            };
                            self.system_theme.apply(ui.ctx(), self.theme);
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
                        if tools_position == ToolsPosition::Right {
                            self.tools_menu(ui);
                        }
                    });
                });
                if let Some(size) = tools_size {
                    let center = egui::pos2(center_x, row.response.rect.center().y);
                    let rect = Rect::from_center_size(center, size);
                    ui.put(rect, |ui: &mut Ui| self.tools_menu(ui));
                }
            });
    }

    fn tools_menu(&mut self, ui: &mut Ui) -> egui::Response {
        ui.menu_button(self.label(183), |ui| {
            if ui.button(self.label(109)).clicked() {
                ui.close();
                self.open_comparison(&[]);
            }
        })
        .response
    }

    fn format_selector(&mut self, ui: &mut Ui) {
        ComboBox::from_id_salt("output_format")
            .selected_text(self.format.name())
            .width(SELECTOR_WIDTH)
            .show_ui(ui, |ui| {
                theme::accent_selection(ui);
                for format in OutputFormat::ALL {
                    ui.selectable_value(&mut self.format, format, format.name());
                }
            })
            .response
            .on_hover_text(self.label(73));
    }

    fn quality_selector(&mut self, ui: &mut Ui) {
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
            .width(SELECTOR_WIDTH)
            .show_ui(ui, |ui| {
                theme::accent_selection(ui);
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

    fn pending_count(&self) -> usize {
        self.queue
            .iter()
            .filter(|entry| matches!(entry.status, Status::Pending))
            .count()
    }

    fn footer(&mut self, ui: &mut Ui) {
        const FOOTER_HEIGHT: f32 = ADD_IMAGES_SIZE.y + 12.0;
        egui::Panel::bottom("queue_footer")
            .exact_size(FOOTER_HEIGHT)
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(20, 0)),
            )
            .show(ui, |ui| {
                let (strip, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), FOOTER_HEIGHT),
                    egui::Sense::hover(),
                );
                let busy = self.worker.is_some();
                let button = Rect::from_center_size(strip.center(), ADD_IMAGES_SIZE);
                let add = ui.put(button, |ui: &mut Ui| {
                    add_images_button(ui, self.label(150), !busy)
                });
                self.show_add_menu(&add);
                let pending = self.pending_count();
                let estimate = self
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.estimate_seconds(&self.queue));
                if pending > 0 || estimate.is_some() {
                    let left =
                        Rect::from_min_max(strip.min, egui::pos2(button.left(), strip.bottom()));
                    let mut label = format!("{}  {pending}", self.label(68));
                    if let Some(seconds) = estimate {
                        label.push_str(&format!(" / ≈ {}", self.catalog.duration(seconds)));
                    }
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(left)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| ui.weak(label),
                    );
                }
                let spinner = Rect::from_min_max(
                    egui::pos2(strip.right() - ADD_IMAGES_SIZE.x, strip.top()),
                    strip.max,
                );
                if busy || self.scans_pending > 0 {
                    ui.put(spinner, egui::Spinner::new());
                }
                let error_area = Rect::from_min_max(
                    egui::pos2(button.right(), strip.top()),
                    egui::pos2(spinner.left() - 4.0, strip.bottom()),
                );
                self.conversion_errors(ui, error_area);
            });
    }

    fn conversion_errors(&mut self, ui: &mut Ui, area: Rect) {
        let open = std::mem::take(&mut self.open_batch_errors);
        let errors =
            self.queue
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| match &entry.status {
                    Status::Done(Err(error)) => Some((index, entry.input.path.as_path(), error)),
                    _ => None,
                });
        let count = errors.clone().count();
        if count == 0 {
            return;
        }
        let button = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(area)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let button = icons::errors(ui, self.label(124));
                    ui.colored_label(ui.visuals().error_fg_color, count.to_string());
                    button
                },
            )
            .inner;
        let id = ui.make_persistent_id("conversion_errors_popup");
        if open {
            egui::Popup::open_id(ui.ctx(), id);
        }
        let jump = egui::Popup::from_toggle_button_response(&button)
            .id(id)
            .align(egui::RectAlign::TOP_END)
            .gap(6.0)
            .frame(egui::Frame::popup(ui.style()).fill(ui.visuals().panel_fill))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.error_panel(ui, &errors.collect::<Vec<_>>()))
            .and_then(|response| response.inner);
        if let Some(index) = jump {
            self.queue_jump = Some(index);
            ui.ctx().request_repaint();
        }
    }

    fn error_panel(&self, ui: &mut Ui, errors: &[(usize, &Path, &Failure)]) -> Option<usize> {
        const WIDTH: f32 = 520.0;
        const FILE_WIDTH: f32 = 170.0;
        const ROW_HEIGHT: f32 = 28.0;
        ui.set_width(WIDTH);
        ui.horizontal(|ui| {
            ui.strong(self.label(124));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::share(ui, self.label(125)).clicked() {
                    let text = errors
                        .iter()
                        .map(|(_, path, error)| {
                            format!("{}\t{}", file_name(path), self.error_description(error))
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(text);
                }
            });
        });
        ui.separator();
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.style_mut().interaction.selectable_labels = false;
        ScrollArea::vertical()
            .max_height(224.0)
            .min_scrolled_height(0.0)
            .show_rows(ui, ROW_HEIGHT, errors.len(), |ui, rows| {
                let mut selected = None;
                for &(index, path, error) in &errors[rows] {
                    let (row, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), ROW_HEIGHT),
                        egui::Sense::click(),
                    );
                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            format!("{} · {}", file_name(path), self.error_summary(error)),
                        )
                    });
                    if response.hovered() || response.has_focus() {
                        ui.painter()
                            .rect_filled(row, 0, ui.visuals().widgets.hovered.weak_bg_fill);
                    }
                    if response.clicked() {
                        selected = Some(index);
                        ui.close();
                    }
                    let split = row.left() + FILE_WIDTH;
                    let file = Rect::from_x_y_ranges(row.left()..=split, row.y_range());
                    let reason = Rect::from_x_y_ranges(split..=row.right(), row.y_range());
                    queue_cell(ui, file, |ui| {
                        ui.add(egui::Label::new(file_name(path)).truncate())
                            .on_hover_text(path_label(path));
                    });
                    queue_cell(ui, reason, |ui| self.show_error(ui, error));
                }
                selected
            })
            .inner
    }

    fn show_convert(&mut self, ui: &mut Ui) {
        let busy = self.worker.is_some();
        let pending = self.pending_count();
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
                match &self.folder {
                    Some(path) => icons::output_folder(ui, file_name(path), !busy),
                    None => ui.add_enabled(!busy, Button::new(self.label(9))),
                }
            })
            .on_hover_text(destination)
            .clicked()
        {
            self.pick_output_folder(ui.ctx());
        }
        if ui
            .put(center, |ui: &mut Ui| {
                if let Some(worker) = &self.worker {
                    ui.add_enabled(
                        !worker.cancelled.load(Ordering::Relaxed),
                        Button::new(RichText::new(self.label(130)).color(egui::Color32::WHITE))
                            .fill(egui::Color32::from_rgb(175, 40, 50)),
                    )
                } else {
                    ui.add_enabled(
                        pending > 0 && self.scans_pending == 0,
                        Button::new(RichText::new(self.label(22)).color(theme::ACCENT_TEXT))
                            .fill(theme::CONVERT_ACCENT),
                    )
                }
            })
            .clicked()
        {
            if let Some(worker) = &self.worker {
                worker.cancelled.store(true, Ordering::Relaxed);
            } else if self.folder.is_some() || self.pick_output_folder(ui.ctx()) {
                self.start_conversion(ui.ctx());
            }
        }
        let busy = self.worker.is_some();
        if ui
            .put(center.translate(offset), |ui: &mut Ui| {
                ui.add_enabled(
                    !busy
                        && self.scans_pending == 0
                        && (!self.queue.is_empty() || self.folder.is_some()),
                    Button::new(self.label(23)),
                )
            })
            .clicked()
        {
            self.queue.clear();
            self.queue_jump = None;
            self.batch_summary = Summary::default();
            self.completed_summary = None;
            self.folder = None;
            self.folder_failure = None;
            self.folder_request = None;
            ui.ctx().request_repaint();
        }
        ui.add_space((actions.bottom() + ACTION_GAP - ui.next_widget_position().y).max(0.0));
        if let Some(error) = &self.folder_failure {
            self.show_error(ui, error);
        }
        if let Some(error) = &self.comparison_failure {
            self.show_error(ui, error);
        }
        if self.queue.is_empty() {
            let hint = self.label(if self.scans_pending > 0 { 149 } else { 152 });
            let add = empty_drop_area(ui, hint, self.label(150));
            self.show_add_menu(&add);
        } else {
            self.show_queue(ui, busy);
        }
    }

    fn show_completion(&mut self, context: &Context) {
        let Some(summary) = &self.completed_summary else {
            return;
        };
        let id = egui::Id::unique("conversion_summary");
        let frame = egui::Frame::popup(&context.style_of(context.theme()));
        let footer_gap = f32::from(frame.inner_margin.bottom) + frame.stroke.width;
        let response = egui::Modal::new(id).frame(frame).show(context, |ui| {
            ui.set_width(360.0);
            let header_x = ui.available_rect_before_wrap().x_range();
            let header = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::close(ui, self.label(142)).clicked() {
                    ui.close_kind(egui::UiKind::Modal);
                }
            });
            let title = egui::Label::new(RichText::new(self.label(135)).heading());
            let title_area =
                Rect::from_x_y_ranges(header_x.expand(-28.0), header.response.rect.y_range());
            ui.place(title_area, title);
            ui.add_space(6.0);
            self.show_completion_totals(ui, summary, id.with("table_width"));
            let open_errors = summary.errors > 0
                && ui
                    .button(format!("{} · {}", self.label(124), summary.errors))
                    .clicked();
            let open_folder = summary.folder().is_some_and(|_| {
                let footer = ui.available_rect_before_wrap();
                ui.add_space(footer_gap);
                let button = ui.vertical_centered(|ui| ui.button(self.label(143))).inner;
                let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                ui.painter()
                    .hline(footer.x_range(), button.rect.top() - footer_gap, stroke);
                button.clicked()
            });
            (open_errors, open_folder)
        });
        let (open_errors, open_folder) = response.inner;
        if open_folder && let Some(folder) = summary.folder() {
            self.folder_failure = None;
            self.folder_request = Some(jobs::open_folder(context, folder.to_owned()));
        }
        if open_errors || open_folder || response.should_close() {
            self.completed_summary = None;
            self.open_batch_errors = open_errors;
            context.request_repaint();
        }
    }

    fn show_completion_totals(&self, ui: &mut Ui, summary: &Summary, width_id: egui::Id) {
        const SPACING: egui::Vec2 = egui::vec2(16.0, 6.0);
        let width = ui.data(|data| data.get_temp::<f32>(width_id).unwrap_or_default());
        let inset = if width > 0.0 {
            ((ui.available_width() - width) * 0.5).max(SPACING.x * 0.5)
        } else {
            SPACING.x * 0.5
        };
        let mut area = ui.available_rect_before_wrap();
        area.min.x += inset;
        let mut row_dividers = Vec::with_capacity(4);
        let grid = ui
            .scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                egui::Grid::new("summary_totals")
                    .num_columns(2)
                    .spacing(SPACING)
                    .show(ui, |ui| {
                        let top = ui.cursor().top() - SPACING.y * 0.5;
                        ui.label(self.label(136));
                        let files = ui.horizontal(|ui| {
                            ui.label(summary.attempted().to_string());
                            icons::right_arrow(ui);
                            ui.label(summary.converted.to_string());
                        });
                        ui.end_row();
                        row_dividers.push(ui.cursor().top() - SPACING.y * 0.5);
                        ui.label(self.label(137));
                        ui.horizontal(|ui| {
                            ui.label(self.catalog.file_size(summary.source_bytes));
                            icons::right_arrow(ui);
                            ui.label(self.catalog.file_size(summary.output_bytes));
                        });
                        ui.end_row();
                        row_dividers.push(ui.cursor().top() - SPACING.y * 0.5);
                        if summary.source_bytes > 0 {
                            let savings = summary.source_bytes.abs_diff(summary.output_bytes);
                            let percent =
                                (savings as f64 / summary.source_bytes as f64 * 100.0).round();
                            let label = if summary.output_bytes > summary.source_bytes {
                                139
                            } else {
                                138
                            };
                            ui.label(self.label(label));
                            ui.strong(format!(
                                "{} ({percent:.0}%)",
                                self.catalog.file_size(savings)
                            ));
                            ui.end_row();
                            row_dividers.push(ui.cursor().top() - SPACING.y * 0.5);
                        }
                        ui.label(self.label(140));
                        ui.label(self.catalog.elapsed(summary.elapsed.as_secs()));
                        ui.end_row();
                        if summary.skipped > 0 {
                            row_dividers.push(ui.cursor().top() - SPACING.y * 0.5);
                            ui.label(self.label(141));
                            ui.label(summary.skipped.to_string());
                            ui.end_row();
                        }
                        let bottom = ui.cursor().top() - SPACING.y * 0.5;
                        (files.response.rect.left(), top..=bottom)
                    })
            })
            .inner;
        let body = grid.response.rect;
        if (width - body.width()).abs() > 0.5 {
            ui.data_mut(|data| data.insert_temp(width_id, body.width()));
            ui.request_discard("measure completion table width");
        }

        let (values_x, rows) = grid.inner;
        let border = Rect::from_x_y_ranges(body.x_range().expand(SPACING.x * 0.5), rows);
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter()
            .rect_stroke(border, 0.0, stroke, egui::StrokeKind::Inside);
        ui.painter()
            .vline(values_x - SPACING.x * 0.5, border.y_range(), stroke);
        for row in row_dividers {
            ui.painter().hline(border.x_range(), row, stroke);
        }
    }

    fn output_dialog_directory(&self) -> Option<&Path> {
        self.folder.as_deref().or_else(|| {
            self.queue
                .first()?
                .input
                .path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
        })
    }

    fn pick_output_folder(&mut self, context: &Context) -> bool {
        let mut dialog = rfd::FileDialog::new();
        if let Some(directory) = self.output_dialog_directory() {
            dialog = dialog.set_directory(directory);
        }
        let Some(folder) = dialog.pick_folder() else {
            return false;
        };
        self.folder = Some(folder);
        self.folder_failure = None;
        context.request_repaint();
        true
    }

    fn start_conversion(&mut self, context: &Context) {
        let Some(folder) = self.folder.clone() else {
            return;
        };
        self.folder_failure = None;
        let entries: Vec<_> = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.status, Status::Pending))
            .map(|(index, entry)| (index, entry.input.clone()))
            .collect();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.worker = Some(Batch {
            queue_end: self.queue.len(),
            estimate: Estimate::default(),
            events: jobs::convert(
                context,
                entries,
                Some(folder),
                Options {
                    format: self.format,
                    auto_resize: false,
                },
                self.mode,
                Arc::clone(&cancelled),
            ),
            cancelled,
        });
    }

    fn show_queue(&mut self, ui: &mut Ui, busy: bool) {
        const HEADER_HEIGHT: f32 = 30.0;
        const ROW_HEIGHT: f32 = 42.0;
        const MIN_COLUMN_WIDTH: f32 = 100.0;
        const ROW_ACTION_SIZE: egui::Vec2 = egui::vec2(28.0, 28.0);
        const FOLDER_GAP: f32 = 6.0;
        const INFO_GAP: f32 = 6.0;

        let content_height = HEADER_HEIGHT + ROW_HEIGHT * self.queue.len() as f32;
        let (bounds, _) = ui.allocate_exact_size(
            egui::vec2(
                ui.available_width(),
                content_height.min(ui.available_height()),
            ),
            egui::Sense::hover(),
        );
        let scroll_style = egui::style::ScrollStyle::solid();
        let scrolls = content_height > bounds.height();
        let mut table = bounds;
        if scrolls {
            table.max.x -= scroll_style.allocated_width();
        }
        let action_width = (table.width() * 0.1).max(2.0 * ROW_ACTION_SIZE.x);
        let output_end = table.width() - action_width;
        let min_width = MIN_COLUMN_WIDTH.min(output_end * 0.5);
        let file_width =
            (table.width() * self.file_column_fraction).clamp(min_width, output_end - min_width);
        let split = table.left() + file_width;
        let output_end_x = table.left() + output_end;
        let preview_end = (output_end_x + table.right()) * 0.5;
        let column_rects = |row: Rect| {
            [
                Rect::from_x_y_ranges(row.left()..=split, row.y_range()),
                Rect::from_x_y_ranges(split..=output_end_x, row.y_range()),
                Rect::from_x_y_ranges(output_end_x..=preview_end, row.y_range()),
                Rect::from_x_y_ranges(preview_end..=table.right(), row.y_range()),
            ]
        };
        let folder_rect = |cell: Rect| {
            Rect::from_center_size(
                egui::pos2(
                    cell.right() - FOLDER_GAP - ROW_ACTION_SIZE.x * 0.5,
                    cell.center().y,
                ),
                ROW_ACTION_SIZE,
            )
        };
        let info_rect = |cell: Rect| {
            Rect::from_center_size(
                egui::pos2(
                    cell.left() + INFO_GAP + ROW_ACTION_SIZE.x * 0.5,
                    cell.center().y,
                ),
                ROW_ACTION_SIZE,
            )
        };
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let header = Rect::from_min_size(
            table.min,
            egui::vec2(output_end, HEADER_HEIGHT.min(table.height())),
        );
        ui.painter()
            .rect_filled(header, 0, ui.visuals().faint_bg_color);
        for (cell, title) in column_rects(header).into_iter().zip([28, 76]) {
            queue_cell(ui, cell, |ui| {
                ui.strong(self.label(title));
            });
        }
        let body = Rect::from_min_max(header.left_bottom(), table.max);
        let mut remove = None;
        let mut compare = None;
        let mut reveal_path = None;
        let mut info_path = None;
        let jump = self.queue_jump.take();
        let scroll_bounds = Rect::from_min_max(body.min, bounds.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(scroll_bounds), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.spacing_mut().scroll = scroll_style;
            let mut scroll_area = ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(body.height())
                .min_scrolled_height(0.0)
                .scroll_bar_visibility(if scrolls {
                    ScrollBarVisibility::AlwaysVisible
                } else {
                    ScrollBarVisibility::AlwaysHidden
                });
            if let Some(index) = jump {
                let center = (index as f32 + 0.5) * ROW_HEIGHT;
                let max_offset = (ROW_HEIGHT * self.queue.len() as f32 - body.height()).max(0.0);
                let offset = (center - body.height() * 0.5).clamp(0.0, max_offset);
                scroll_area = scroll_area.vertical_scroll_offset(offset);
            }
            scroll_area.id_salt("queue_rows").show_rows(
                ui,
                ROW_HEIGHT,
                self.queue.len(),
                |ui, rows| {
                    ui.set_clip_rect(ui.clip_rect().intersect(body));
                    for index in rows {
                        let entry = &self.queue[index];
                        ui.push_id(index, |ui| {
                            let (row, _) = ui.allocate_exact_size(
                                egui::vec2(body.width(), ROW_HEIGHT),
                                egui::Sense::hover(),
                            );
                            if index % 2 == 1 {
                                ui.painter()
                                    .rect_filled(row, 0, ui.visuals().faint_bg_color);
                            }
                            let [mut file, mut output, preview, delete] = column_rects(row);
                            let input_info = info_rect(file);
                            if ui
                                .put(input_info, |ui: &mut Ui| icons::about(ui, self.label(184)))
                                .clicked()
                            {
                                info_path = Some(entry.input.path.clone());
                            }
                            let input_folder = folder_rect(file);
                            file.min.x = input_info.right();
                            file.max.x = input_folder.left();
                            queue_cell(ui, file, |ui| {
                                let show_name = |ui: &mut Ui| {
                                    ui.add(
                                        egui::Label::new(file_name(&entry.input.path)).truncate(),
                                    )
                                    .on_hover_text(path_label(&entry.input.path));
                                };
                                if let Status::Done(Err(error)) = &entry.status {
                                    ui.vertical(|ui| {
                                        show_name(ui);
                                        self.show_error(ui, error);
                                    });
                                } else {
                                    show_name(ui);
                                }
                            });
                            if ui
                                .put(input_folder, |ui: &mut Ui| {
                                    icons::reveal_file(ui, self.label(134))
                                })
                                .clicked()
                            {
                                reveal_path = Some(entry.input.path.clone());
                            }
                            let conversion = match &entry.status {
                                Status::Done(Ok(Some(result))) => Some(result),
                                _ => None,
                            };
                            let output_folder = folder_rect(output);
                            if let Some(conversion) = conversion {
                                let output_info = info_rect(output);
                                if ui
                                    .put(output_info, |ui: &mut Ui| {
                                        icons::about(ui, self.label(184))
                                    })
                                    .clicked()
                                {
                                    info_path = Some(conversion.path.clone());
                                }
                                output.min.x = output_info.right();
                                output.max.x = output_folder.left();
                            }
                            queue_cell(ui, output, |ui| {
                                self.show_output(ui, &entry.status, &mut reveal_path);
                            });
                            if let Some(conversion) = conversion
                                && ui
                                    .put(output_folder, |ui: &mut Ui| {
                                        icons::reveal_file(ui, self.label(70))
                                    })
                                    .clicked()
                            {
                                reveal_path = Some(conversion.path.clone());
                            }
                            let compare_rect =
                                Rect::from_center_size(preview.center(), ROW_ACTION_SIZE);
                            if ui
                                .put(compare_rect, |ui: &mut Ui| {
                                    icons::compare(ui, self.label(122), conversion.is_some())
                                })
                                .clicked()
                                && let Some(conversion) = conversion
                            {
                                compare = Some([entry.input.path.clone(), conversion.path.clone()]);
                            }
                            let remove_rect =
                                Rect::from_center_size(delete.center(), ROW_ACTION_SIZE);
                            if ui
                                .put(remove_rect, |ui: &mut Ui| {
                                    remove_file_button(ui, self.label(121), !busy)
                                })
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            ui.painter().hline(row.x_range(), row.bottom(), stroke);
                        });
                    }
                },
            );
        });
        ui.painter()
            .rect_stroke(header, 0, stroke, egui::StrokeKind::Inside);
        ui.painter()
            .rect_stroke(body, 0, stroke, egui::StrokeKind::Inside);
        ui.painter().vline(output_end_x, body.y_range(), stroke);
        ui.painter().vline(preview_end, body.y_range(), stroke);
        let divider = ui
            .interact(
                Rect::from_center_size(
                    egui::pos2(split, table.center().y),
                    egui::vec2(10.0, table.height()),
                ),
                ui.scope_id().with("queue_column_divider"),
                egui::Sense::drag(),
            )
            .on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal);
        let divider_stroke = if divider.hovered() || divider.dragged() {
            ui.visuals().widgets.hovered.fg_stroke
        } else {
            stroke
        };
        ui.painter().vline(split, table.y_range(), divider_stroke);
        if divider.dragged() {
            self.file_column_fraction = (file_width + divider.drag_delta().x)
                .clamp(min_width, output_end - min_width)
                / table.width();
            ui.ctx().request_repaint();
        }
        // The current batch addresses its entries by index. Only remove rows after it completes.
        if let Some(index) = remove {
            self.queue.remove(index);
            ui.ctx().request_repaint();
        }
        if let Some(paths) = compare {
            self.open_comparison(&paths);
        }
        if let Some(path) = info_path {
            self.image_info.open(path);
        }
        if let Some(path) = reveal_path {
            self.folder_failure = None;
            self.folder_request = Some(jobs::reveal_file(ui.ctx(), path));
        }
    }

    fn show_output(&self, ui: &mut Ui, status: &Status, reveal_path: &mut Option<PathBuf>) {
        match status {
            Status::Done(Ok(Some(result))) => {
                let grew = result.bytes > result.source_bytes;
                let mut text_area = ui.available_rect_before_wrap();
                if grew {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    text_area.max.x -= icons::INFO_SIZE.x + ui.spacing().item_spacing.x;
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(text_area), |ui| {
                    ui.add(
                        egui::Label::new(format!(
                            "{} -> {}",
                            self.catalog.file_size(result.source_bytes as u64),
                            self.catalog.file_size(result.bytes as u64),
                        ))
                        .truncate()
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text(path_label(&result.path))
                    .context_menu(|ui| {
                        if ui.button(self.label(70)).clicked() {
                            *reveal_path = Some(result.path.clone());
                            ui.close();
                        }
                    });
                });
                if grew {
                    icons::info(ui, self.label(129));
                }
            }
            Status::Done(Ok(None)) => {
                ui.weak("—").on_hover_text(self.label(107));
            }
            _ => {
                ui.weak("—");
            }
        }
    }

    fn show_add_menu(&mut self, button: &egui::Response) {
        let selection = egui::Popup::menu(button).show(|ui| {
            if ui.button(self.label(151)).clicked() {
                ui.close();
                rfd::FileDialog::new()
                    .add_filter(self.label(65), jobs::IMAGE_EXTENSIONS)
                    .pick_files()
            } else if ui.button(self.label(144)).clicked() {
                ui.close();
                rfd::FileDialog::new().pick_folder().map(|path| vec![path])
            } else {
                None
            }
        });
        if let Some(paths) = selection.and_then(|menu| menu.inner) {
            self.scan_paths(paths);
        }
    }

    fn scan_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.scan_requests.send(paths).is_ok() {
            self.scans_pending += 1;
        } else {
            self.input_scanner_stopped();
        }
    }

    fn input_scanner_stopped(&mut self) {
        self.scans_pending = 0;
        self.folder_failure = Some(Failure {
            label: 148,
            detail: String::new(),
        });
    }

    fn poll_scans(&mut self) {
        while self.scans_pending > 0 {
            match self.scanned_inputs.try_recv() {
                Ok(scanned) => {
                    self.scans_pending -= 1;
                    self.add_inputs(scanned.files);
                    if let Some(failure) = scanned.failure {
                        self.folder_failure = Some(failure);
                    }
                    self.finish_summary();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.input_scanner_stopped(),
            }
        }
    }

    fn add_inputs(&mut self, files: impl IntoIterator<Item = InputFile>) {
        let mut known: HashSet<_> = self
            .queue
            .iter()
            .map(|entry| entry.input.path.clone())
            .collect();
        let entries: Vec<_> = files
            .into_iter()
            .filter(|input| known.insert(input.path.clone()))
            .map(|input| QueueEntry {
                input,
                size: Arc::new(OnceLock::new()),
                status: Status::Pending,
            })
            .collect();
        let probes = entries
            .iter()
            .map(|entry| InputProbe {
                path: entry.input.path.clone(),
                result: Arc::downgrade(&entry.size),
            })
            .collect();
        self.queue.extend(entries);
        let _ = self.input_probes.send(probes);
    }

    fn finish_summary(&mut self) {
        if self.worker.is_none()
            && self.scans_pending == 0
            && self.pending_count() == 0
            && self.batch_summary.attempted() > 0
        {
            self.completed_summary = Some(std::mem::take(&mut self.batch_summary));
        }
    }

    fn poll_worker(&mut self) {
        while let Some(worker) = &mut self.worker {
            match worker.events.try_recv() {
                Ok(Event::Started(index)) => self.queue[index].status = Status::Running,
                Ok(Event::Converted(index, result)) => {
                    let result = result.map(|completed| {
                        worker.estimate.record(completed.timing);
                        completed.output
                    });
                    self.batch_summary.record(&result);
                    self.queue[index].status = Status::Done(result);
                }
                Ok(Event::Interrupted(index)) => self.queue[index].status = Status::Pending,
                Ok(Event::Finished(elapsed)) => {
                    self.batch_summary.elapsed += elapsed;
                    self.worker = None;
                    self.finish_summary();
                }
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

    fn open_comparison(&mut self, paths: &[PathBuf]) {
        self.comparison_failure = None;
        let child = std::env::current_exe().and_then(|executable| {
            Command::new(executable)
                .args(["--compare", &self.language, theme_name(self.theme)])
                .args(paths)
                .env(
                    comparison::TITLE_CASE_ENV,
                    comparison::title_case_code(self.window_size.uppercase_title()),
                )
                .stdin(Stdio::piped())
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
        ui.add(
            egui::Label::new(
                RichText::new(self.error_summary(error)).color(ui.visuals().error_fg_color),
            )
            .truncate(),
        )
        .on_hover_text(self.error_description(error));
    }

    fn error_summary(&self, error: &Failure) -> String {
        format!("[E{}] {}", error.label, self.label(error.label))
    }

    fn error_description(&self, error: &Failure) -> String {
        let summary = self.error_summary(error);
        if error.detail.is_empty() {
            summary
        } else {
            format!("{summary}\n{}", error.detail)
        }
    }
}

impl eframe::App for Tenka {
    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        let context = &ui.ctx().clone();
        self.window_size.update(context);
        self.system_theme.apply(context, self.theme);
        self.scan_paths(take_dropped_paths(context));
        self.poll_scans();
        self.poll_worker();
        if let Some(result) = self
            .folder_request
            .as_ref()
            .and_then(|request| request.try_recv().ok())
        {
            self.folder_failure = result.err();
            self.folder_request = None;
        }
        self.header(ui);
        if !self.queue.is_empty() {
            self.footer(ui);
        }
        let mut margin = egui::Margin::symmetric(DROP_AREA_SIDE_MARGIN, ACTION_GAP as i8);
        if self.queue.is_empty() {
            margin.bottom = margin.left;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(margin))
            .show(ui, |ui| self.show_convert(ui));
        self.show_completion(context);
        let previous_title = self.window_size.uppercase_title();
        self.about
            .show(context, &self.catalog, &mut self.window_size);
        let uppercase = self.window_size.uppercase_title();
        if uppercase != previous_title {
            context.send_viewport_cmd(egui::ViewportCommand::Title(app_title(uppercase).into()));
            for child in &mut self.comparison_windows {
                if let Some(input) = &mut child.stdin {
                    let _ = input.write_all(comparison::title_case_code(uppercase).as_bytes());
                }
            }
        }
        self.image_info.show(context, &self.catalog);
        self.poll_comparison_windows(context);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("language", self.language.clone());
        storage.set_string("theme", theme_name(self.theme).into());
        storage.set_string("output_format", self.format.extension().into());
        let mode = match self.mode {
            OptimizationMode::Best => "best",
            OptimizationMode::Balance => "balance",
            OptimizationMode::Cheap => "cheap",
        };
        storage.set_string("optimization_mode", mode.into());
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

fn empty_drop_area(ui: &mut Ui, hint: &str, button_label: &str) -> egui::Response {
    let (area, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    ui.painter().rect_stroke(
        area,
        0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            let content_height = ui.text_style_height(&egui::TextStyle::Body)
                + ui.spacing().item_spacing.y
                + ADD_IMAGES_SIZE.y;
            ui.add_space(((area.height() - content_height) * 0.5).max(0.0));
            ui.label(hint);
            add_images_button(ui, button_label, true)
        },
    )
    .inner
}

fn add_images_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        Button::new(RichText::new("+").size(20.0)).min_size(ADD_IMAGES_SIZE),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response.on_hover_text(label)
}

fn queue_cell(ui: &mut Ui, cell: Rect, contents: impl FnOnce(&mut Ui)) {
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(cell.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(cell));
            contents(ui);
        },
    );
}

fn remove_file_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(enabled, Button::new("").frame_when_inactive(false));
    let center = response.rect.center();
    let mut color = ui.visuals().error_fg_color;
    if !enabled {
        color = color.gamma_multiply(0.45);
    }
    let stroke = egui::Stroke::new(1.8_f32, color);
    for offset in [egui::vec2(5.0, 5.0), egui::vec2(5.0, -5.0)] {
        ui.painter()
            .line_segment([center - offset, center + offset], stroke);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response.on_hover_text(label)
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event as InputEvent, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Shape};

    pub(super) fn click(pos: Pos2) -> Vec<InputEvent> {
        let button = |pressed| InputEvent::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        vec![InputEvent::PointerMoved(pos), button(true), button(false)]
    }

    pub(super) fn render_ui(
        context: &Context,
        input: RawInput,
        draw: impl FnMut(&mut Ui),
    ) -> FullOutput {
        let mut output = context.run_ui(input, draw);
        output.textures_delta.clear();
        output
    }

    fn text_position(output: &FullOutput, label: &str) -> Option<Pos2> {
        output.shapes.iter().find_map(|shape| {
            let Shape::Text(text) = &shape.shape else {
                return None;
            };
            let center = text.pos + text.galley.rect.center().to_vec2();
            (text.galley.text() == label && shape.clip_rect.contains(center)).then_some(center)
        })
    }

    #[test]
    fn conversion_choices_restore_on_startup_and_default_for_old_settings() {
        use eframe::{App as _, Storage};

        #[derive(Default)]
        struct MemoryStorage(std::collections::HashMap<String, String>);

        impl Storage for MemoryStorage {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }

            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.into(), value);
            }

            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }

            fn flush(&mut self) {}
        }

        let start = |storage: &dyn Storage| {
            let mut creation = eframe::CreationContext::_new_kittest(Context::default());
            creation.storage = Some(storage);
            Tenka::new(&creation)
        };
        let mut storage = MemoryStorage::default();
        storage.set_string("language", "en".into());
        let mut app = start(&storage);
        assert_eq!(app.mode, OptimizationMode::Best);
        assert_eq!(app.format, OutputFormat::JpegXl);

        app.mode = OptimizationMode::Cheap;
        app.format = OutputFormat::Webp;
        app.save(&mut storage);
        let restarted = start(&storage);
        assert_eq!(restarted.mode, OptimizationMode::Cheap);
        assert_eq!(restarted.format, OutputFormat::Webp);
        assert_eq!(restarted.language, "en");
    }

    #[test]
    fn output_dialog_starts_at_the_first_remaining_file() {
        let context = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(context));
        assert_eq!(app.output_dialog_directory(), None);
        app.add_inputs(
            ["/test/first/a.jpg", "/test/second/b.png"]
                .map(|path| InputFile::from(PathBuf::from(path))),
        );
        assert_eq!(
            app.output_dialog_directory(),
            Some(Path::new("/test/first"))
        );
        app.queue.remove(0);
        assert_eq!(
            app.output_dialog_directory(),
            Some(Path::new("/test/second"))
        );
        app.folder = Some(PathBuf::from("/test/output"));
        assert_eq!(
            app.output_dialog_directory(),
            Some(Path::new("/test/output"))
        );
    }

    #[test]
    fn about_opens_from_the_header_and_hides_window_settings_behind_the_logo() {
        use eframe::App as _;

        let ctx = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.catalog = Catalog::new("en");
        app.theme = ThemePreference::System;
        let mut render = |events| {
            render_ui(
                &ctx,
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, MAIN_WINDOW_SIZE)),
                    events,
                    ..Default::default()
                },
                |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
            )
        };
        let check_toolbar = |frame: &FullOutput, position| {
            let center = MAIN_WINDOW_SIZE.x * 0.5;
            let quality = text_position(frame, "Best").unwrap().x;
            let format = text_position(frame, "JPEG XL").unwrap().x;
            let tools = text_position(frame, "Tools").unwrap().x;
            assert!(quality < format && format < center);
            match position {
                ToolsPosition::Left => assert!(format < tools && tools < center),
                ToolsPosition::Center => assert!((tools - center).abs() <= 1.0),
                ToolsPosition::Right => {
                    let globe = frame
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            Shape::Circle(circle) if circle.radius == 8.0 => Some(circle.center.x),
                            _ => None,
                        })
                        .unwrap();
                    assert!(tools > center && tools < globe && globe - tools < 70.0);
                }
            }
        };
        let main = render(Vec::new());
        assert!(text_position(&main, "Tenka").is_none());
        check_toolbar(&main, ToolsPosition::Right);
        let drop_area = main
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect)
                    if rect.rect.width() > 700.0
                        && rect.rect.height() > 300.0
                        && rect.fill == egui::Color32::TRANSPARENT
                        && !rect.stroke.is_empty() =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        let margin = f32::from(DROP_AREA_SIDE_MARGIN);
        assert!((drop_area.left() - margin).abs() <= 1.0);
        assert!((MAIN_WINDOW_SIZE.x - drop_area.right() - margin).abs() <= 1.0);
        assert!((MAIN_WINDOW_SIZE.y - drop_area.bottom() - margin).abs() <= 1.0);
        let info = main
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Circle(circle) if circle.radius == 7.0 => Some(circle.center),
                _ => None,
            })
            .unwrap();
        assert!(info.x > text_position(&main, "A").unwrap().x);
        render(click(info));
        for _ in 0..3 {
            render(Vec::new());
        }
        let about = render(Vec::new());
        for label in [
            "Repository",
            "GitHub",
            "License",
            env!("CARGO_PKG_VERSION"),
            "Check for updates",
            "Component",
            "Upstream",
            "Integration details",
            "libjxl/libjxl",
        ] {
            assert!(text_position(&about, label).is_some(), "{label}");
        }
        assert!(text_position(&about, "About").is_none());

        let logo = about
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect.center()),
                _ => None,
            })
            .unwrap();
        for _ in 0..4 {
            render(click(logo));
            assert!(text_position(&render(Vec::new()), "Secret menu").is_none());
        }
        render(click(logo));
        render(Vec::new());
        let settings = render(Vec::new());
        for label in [
            "Secret menu",
            "Width",
            "Height",
            "760",
            "620",
            "Use uppercase app title",
            "Center Tools",
            "Tools on the left",
            "Reset",
        ] {
            assert!(text_position(&settings, label).is_some(), "{label}");
        }
        assert!(text_position(&settings, "Quality and format on the left").is_none());
        for (option, position) in [
            ("Center Tools", ToolsPosition::Center),
            ("Tools on the left", ToolsPosition::Left),
            ("Tools on the left", ToolsPosition::Right),
            ("Tools on the left", ToolsPosition::Left),
            ("Center Tools", ToolsPosition::Center),
            ("Center Tools", ToolsPosition::Right),
            ("Tools on the left", ToolsPosition::Left),
        ] {
            let frame = render(Vec::new());
            render(click(text_position(&frame, option).unwrap()));
            check_toolbar(&render(Vec::new()), position);
        }
        render(click(text_position(&settings, "760").unwrap()));
        render(Vec::new());
        render(vec![InputEvent::Text("1040".into())]);
        let resized = render(vec![InputEvent::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(resized.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::InnerSize(size) if *size == egui::vec2(1040.0, 620.0))));

        let unlock = text_position(&render(Vec::new()), "Unlock window resizing").unwrap();
        let output = render(click(unlock));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Resizable(false)))
        );

        let title_toggle = text_position(&render(Vec::new()), "Use uppercase app title").unwrap();
        let output = render(click(title_toggle));
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Title(title) if title == "TENKA")));

        let reset = text_position(&render(Vec::new()), "Reset").unwrap();
        let output = render(click(reset));
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Resizable(true)))
        );
        assert!(commands.iter().any(|command| matches!(command, egui::ViewportCommand::InnerSize(size) if *size == MAIN_WINDOW_SIZE)));
        assert!(commands.iter().any(
            |command| matches!(command, egui::ViewportCommand::Title(title) if title == "Tenka")
        ));

        check_toolbar(&render(Vec::new()), ToolsPosition::Right);

        render(click(Pos2::new(10.0, 10.0)));
        assert!(text_position(&render(Vec::new()), "Secret menu").is_some());
        let title_toggle = text_position(&render(Vec::new()), "Use uppercase app title").unwrap();
        render(click(title_toggle));
        render(vec![InputEvent::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        let about = render(Vec::new());
        assert!(text_position(&about, "Repository").is_some());
        assert!(text_position(&about, "TENKA").is_some());
    }

    #[test]
    fn the_add_menu_and_dropping_a_folder_work_in_the_conversion_window() {
        use eframe::App as _;

        #[derive(Debug)]
        struct FolderDrop(PathBuf);

        impl egui::DroppedFile for FolderDrop {
            fn path(&self) -> &Path {
                &self.0
            }

            fn bytes(&self) -> Result<Vec<u8>, String> {
                Err("This drop is a folder".into())
            }
        }

        let ctx = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.catalog = Catalog::new("en");
        let render = |app: &mut Tenka, events, dropped_files| {
            render_ui(
                &ctx,
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, MAIN_WINDOW_SIZE)),
                    events,
                    dropped_files,
                    ..Default::default()
                },
                |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()),
            )
        };

        let empty = render(&mut app, Vec::new(), Vec::new());
        let add = text_position(&empty, "+").unwrap();
        render(&mut app, click(add), Vec::new());
        let menu = render(&mut app, Vec::new(), Vec::new());
        assert!(text_position(&menu, "Add files").is_some());
        assert!(text_position(&menu, "Add folder").is_some());

        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join("nested/image.png");
        std::fs::create_dir(input.parent().unwrap()).unwrap();
        std::fs::write(&input, []).unwrap();
        render(
            &mut app,
            Vec::new(),
            vec![Arc::new(FolderDrop(folder.path().to_owned()))],
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while app.queue.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            render(&mut app, Vec::new(), Vec::new());
        }
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.queue[0].input.path, input);
    }

    #[test]
    fn queue_folder_icons_are_inside_the_file_and_output_columns() {
        let ctx = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.queue.push(QueueEntry {
            input: InputFile::from(PathBuf::from("source.jpg")),
            size: Arc::new(OnceLock::new()),
            status: Status::Done(Ok(Some(Conversion {
                path: PathBuf::from("source.jxl"),
                source_bytes: 2048,
                bytes: 1024,
            }))),
        });
        let rendered = render_ui(
            &ctx,
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, MAIN_WINDOW_SIZE)),
                ..Default::default()
            },
            |ui| app.show_queue(ui, false),
        );
        let mut folders = Vec::new();
        let mut dividers = Vec::new();
        for shape in &rendered.shapes {
            match &shape.shape {
                Shape::Path(path) if path.closed && path.points.len() == 6 => {
                    folders.push(Rect::from_points(&path.points));
                }
                Shape::LineSegment { points: [a, b], .. }
                    if a.x == b.x && (a.y - b.y).abs() >= 40.0 =>
                {
                    dividers.push(a.x);
                }
                _ => {}
            }
        }
        folders.sort_by(|a, b| a.left().total_cmp(&b.left()));
        dividers.sort_by(f32::total_cmp);
        assert_eq!(folders.len(), 2);
        assert_eq!(dividers.len(), 3);
        assert!(folders[0].right() < dividers[0]);
        assert!(folders[1].left() > dividers[0]);
        assert!(folders[1].right() < dividers[1]);
    }

    #[test]
    fn clicking_an_error_scrolls_to_its_row_in_the_queue() {
        let ctx = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.catalog = Catalog::new("en");
        app.queue = (0..32)
            .map(|index| QueueEntry {
                input: InputFile::from(PathBuf::from(format!("image-{index}.jpg"))),
                size: Arc::new(OnceLock::new()),
                status: if index == 31 {
                    Status::Done(Err(Failure {
                        label: 51,
                        detail: String::new(),
                    }))
                } else {
                    Status::Pending
                },
            })
            .collect();
        let mut render = |events| {
            render_ui(
                &ctx,
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, MAIN_WINDOW_SIZE)),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.footer(ui);
                    egui::CentralPanel::default().show(ui, |ui| app.show_queue(ui, false));
                },
            )
        };
        let initial = render(Vec::new());
        assert!(text_position(&initial, "image-31.jpg").is_none());
        let toggle = initial
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Circle(circle) if circle.radius == 9.0 => Some(circle.center),
                _ => None,
            })
            .unwrap();
        render(click(toggle));
        let popup = render(Vec::new());
        let error = text_position(&popup, "[E51] Image exceeds input limits").unwrap();
        render(click(error));
        let queue = render(Vec::new());
        assert!(text_position(&queue, "Conversion errors").is_none());
        assert!(text_position(&queue, "image-31.jpg").is_some());
    }

    #[test]
    fn completion_after_resume_counts_only_this_batch_and_reuses_the_error_panel() {
        let ctx = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.catalog = Catalog::new("en");
        app.add_inputs(
            ["old.jpg", "saved.jpg", "skipped.jpg", "failed.jpg"]
                .map(|name| InputFile::from(PathBuf::from(name))),
        );
        let output = || Conversion {
            path: PathBuf::from("/results/saved.jxl"),
            source_bytes: 2048,
            bytes: 1024,
        };
        app.queue[0].status = Status::Done(Ok(Some(output())));
        let timing = estimate::Timing {
            bytes: 2048,
            pixels: 16,
            read: Duration::ZERO,
            processing: Duration::ZERO,
        };
        let send_batch = |app: &mut Tenka, events, cancelled| {
            let (send, receive) = std::sync::mpsc::channel();
            let events: Vec<Event> = events;
            for event in events {
                send.send(event).unwrap();
            }
            app.worker = Some(Batch {
                events: receive,
                cancelled: Arc::new(AtomicBool::new(cancelled)),
                queue_end: app.queue.len(),
                estimate: Estimate::default(),
            });
            app.poll_worker();
        };

        send_batch(
            &mut app,
            vec![
                Event::Converted(
                    1,
                    Ok(jobs::CompletedConversion {
                        output: Some(output()),
                        timing,
                    }),
                ),
                Event::Started(2),
                Event::Interrupted(2),
                Event::Finished(Duration::from_secs(60)),
            ],
            true,
        );
        assert!(app.completed_summary.is_none());
        send_batch(
            &mut app,
            vec![
                Event::Converted(
                    2,
                    Ok(jobs::CompletedConversion {
                        output: None,
                        timing,
                    }),
                ),
                Event::Converted(
                    3,
                    Err(Failure {
                        label: 51,
                        detail: String::new(),
                    }),
                ),
                Event::Finished(Duration::from_secs(5)),
            ],
            false,
        );
        let summary = app.completed_summary.as_ref().unwrap();
        assert_eq!(summary.attempted(), 3);
        assert_eq!(summary.converted, 1);
        assert_eq!(summary.folder(), Some(Path::new("/results")));

        let mut render = |events| {
            render_ui(
                &ctx,
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, MAIN_WINDOW_SIZE)),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.footer(ui);
                    app.show_completion(ui.ctx());
                },
            )
        };
        render(Vec::new());
        let modal = render(Vec::new());
        for label in [
            "Done",
            "Files",
            "Sizes",
            "Saved",
            "Time",
            "3",
            "1",
            "2.0 KB",
            "1.0 KB",
            "1.0 KB (50%)",
            "1 min 5 s",
            "Skipped",
        ] {
            assert!(text_position(&modal, label).is_some(), "{label}");
        }
        let center = MAIN_WINDOW_SIZE.x * 0.5;
        let folder = text_position(&modal, "Folder").unwrap();
        let heading = text_position(&modal, "Done").unwrap();
        assert!((folder.x - center).abs() < 2.0);
        assert!((heading.x - center).abs() < 2.0);
        let stats = text_position(&modal, "Sizes").unwrap();
        let table = modal
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect)
                    if rect.fill == egui::Color32::TRANSPARENT
                        && !rect.stroke.is_empty()
                        && rect.rect.contains(stats) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        assert!((table.center().x - center).abs() < 2.0, "{table:?}");
        assert!(modal.shapes.iter().any(|shape| {
            matches!(shape.shape, Shape::LineSegment { points: [a, b], .. }
                if a.x == b.x && (a.y - table.top()).abs() < 1.0
                    && (b.y - table.bottom()).abs() < 1.0)
        }));
        let button = modal
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect) if rect.rect.width() < 100.0 && rect.rect.contains(folder) => {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        let divider = modal
            .shapes
            .iter()
            .find_map(|shape| match shape.shape {
                Shape::LineSegment { points: [a, b], .. }
                    if a.y == b.y
                        && a.y > table.bottom()
                        && a.y < button.top()
                        && b.x - a.x > 300.0 =>
                {
                    Some(a.y)
                }
                _ => None,
            })
            .unwrap();
        let frame = ctx
            .memory(|memory| memory.area_rect(egui::Id::unique("conversion_summary")))
            .unwrap();
        let gap_above = button.top() - divider;
        let gap_below = frame.bottom() - button.bottom();
        assert!(
            (gap_above - gap_below).abs() < 2.0,
            "{gap_above} vs {gap_below}"
        );
        let arrow_shafts = modal
            .shapes
            .iter()
            .filter(|shape| {
                matches!(shape.shape, Shape::LineSegment { points: [a, b], .. }
                    if a.y == b.y && (b.x - a.x - 10.0).abs() < 0.1)
            })
            .count();
        assert_eq!(arrow_shafts, 2);
        render(click(
            text_position(&modal, "Conversion errors · 1").unwrap(),
        ));
        render(Vec::new());
        let popup = render(Vec::new());
        assert!(text_position(&popup, "Done").is_none());
        assert!(text_position(&popup, "[E51] Image exceeds input limits").is_some());
    }

    #[test]
    fn completion_cross_in_the_top_right_dismisses_the_dialog() {
        let ctx = Context::default();
        let mut app = Tenka::new(&eframe::CreationContext::_new_kittest(ctx.clone()));
        app.catalog = Catalog::new("en");
        app.completed_summary = Some(Summary::default());
        let mut render = |events| {
            render_ui(
                &ctx,
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, MAIN_WINDOW_SIZE)),
                    events,
                    ..Default::default()
                },
                |ui| app.show_completion(ui.ctx()),
            )
        };

        render(Vec::new());
        let modal = render(Vec::new());
        let heading = text_position(&modal, "Done").unwrap();
        let cross = modal
            .shapes
            .iter()
            .find_map(|shape| match shape.shape {
                Shape::LineSegment { points: [a, b], .. }
                    if b.x - a.x == 9.0 && b.y - a.y == 9.0 =>
                {
                    Some(a.lerp(b, 0.5))
                }
                _ => None,
            })
            .unwrap();
        assert!(cross.x > heading.x && cross.x > MAIN_WINDOW_SIZE.x / 2.0 + 150.0);
        assert!((cross.y - heading.y).abs() < 3.0);
        assert!(text_position(&modal, "Folder").is_none());
        render(click(cross));
        assert!(app.completed_summary.is_none());
    }
}
