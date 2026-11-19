mod comparison;
mod file_drop;
mod icons;
mod jobs;
mod language;
mod theme;

use std::{
    collections::VecDeque,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError},
    },
    time::Duration,
};

use eframe::egui::{
    self, Button, ComboBox, Context, Rect, RichText, ScrollArea, ThemePreference, Ui,
    scroll_area::ScrollBarVisibility,
};
use file_drop::FileDrops;
use jobs::{Conversion, Event, Failure};
use language::Catalog;
use tenka_core::{OptimizationMode, Options, OutputFormat};

const ADD_IMAGES_SIZE: egui::Vec2 = egui::vec2(34.0, 30.0);
const MAIN_WINDOW_SIZE: egui::Vec2 = egui::vec2(960.0, 650.0);

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../packaging/io.github.yrmuq.tenka.png"
    ))
    .expect("the embedded Tenka icon is valid PNG");
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
            .with_app_id("io.github.yrmuq.tenka")
            .with_icon(icon),
        window_builder: Some(Box::new(|window| {
            // Override the size and maximized state saved by resizable versions.
            window
                .with_inner_size(MAIN_WINDOW_SIZE)
                .with_min_inner_size(MAIN_WINDOW_SIZE)
                .with_max_inner_size(MAIN_WINDOW_SIZE)
                .with_maximized(false)
                .with_maximize_button(false)
        })),
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

struct Batch {
    events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    remaining: usize,
    recent: VecDeque<Duration>,
}

impl Batch {
    fn record(&mut self, elapsed: Duration) {
        self.remaining -= 1;
        if self.recent.len() == 10 {
            self.recent.pop_front();
        }
        self.recent.push_back(elapsed);
    }

    fn estimate_seconds(&self) -> Option<u64> {
        if self.remaining == 0 || self.recent.is_empty() || self.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let seconds: f64 = self.recent.iter().map(Duration::as_secs_f64).sum();
        Some((seconds * self.remaining as f64 / self.recent.len() as f64).ceil() as u64)
    }
}

struct Tenka {
    language: String,
    theme: ThemePreference,
    system_theme: theme::SystemTheme,
    file_drops: FileDrops,
    catalog: Catalog,
    queue: Vec<QueueEntry>,
    file_column_fraction: f32,
    folder: Option<PathBuf>,
    format: OutputFormat,
    mode: OptimizationMode,
    folder_failure: Option<Failure>,
    worker: Option<Batch>,
    comparison_windows: Vec<Child>,
    comparison_failure: Option<Failure>,
}

impl Tenka {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        // Winit creates its Wayland decoration frame after applying startup size hints.
        context
            .egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Resizable(false));
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
            file_column_fraction: 0.5,
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
                        self.open_comparison(&[]);
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

    fn pending_count(&self) -> usize {
        self.queue
            .iter()
            .filter(|entry| matches!(entry.status, Status::Pending))
            .count()
    }

    fn footer(&mut self, context: &Context) {
        const FOOTER_HEIGHT: f32 = ADD_IMAGES_SIZE.y + 12.0;
        egui::TopBottomPanel::bottom("queue_footer")
            .exact_height(FOOTER_HEIGHT)
            .frame(
                egui::Frame::side_top_panel(&context.style())
                    .inner_margin(egui::Margin::symmetric(20, 0)),
            )
            .show(context, |ui| {
                let (strip, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), FOOTER_HEIGHT),
                    egui::Sense::hover(),
                );
                let busy = self.worker.is_some();
                let button = Rect::from_center_size(strip.center(), ADD_IMAGES_SIZE);
                if ui
                    .put(button, |ui: &mut Ui| {
                        add_images_button(ui, &self.catalog, !busy)
                    })
                    .clicked()
                    && let Some(files) = self.picker().pick_files()
                {
                    self.add_files(files);
                }
                let pending = self.pending_count();
                let estimate = self.worker.as_ref().and_then(Batch::estimate_seconds);
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
                if busy {
                    ui.put(spinner, egui::Spinner::new());
                }
                let error_area = Rect::from_min_max(
                    egui::pos2(button.right(), strip.top()),
                    egui::pos2(spinner.left() - 4.0, strip.bottom()),
                );
                self.conversion_errors(ui, error_area);
            });
    }

    fn conversion_errors(&self, ui: &mut Ui, area: Rect) {
        let errors = self.queue.iter().filter_map(|entry| match &entry.status {
            Status::Done(Err(error)) => Some((entry.path.as_path(), error)),
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
        egui::Popup::from_toggle_button_response(&button)
            .id(ui.make_persistent_id("conversion_errors_popup"))
            .align(egui::RectAlign::TOP_END)
            .gap(6.0)
            .frame(egui::Frame::popup(ui.style()).fill(ui.visuals().panel_fill))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| self.error_panel(ui, &errors.collect::<Vec<_>>()));
    }

    fn error_panel(&self, ui: &mut Ui, errors: &[(&Path, &Failure)]) {
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
                        .map(|(path, error)| {
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
        ScrollArea::vertical()
            .max_height(224.0)
            .min_scrolled_height(0.0)
            .show_rows(ui, ROW_HEIGHT, errors.len(), |ui, rows| {
                for (path, error) in &errors[rows] {
                    let (row, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), ROW_HEIGHT),
                        egui::Sense::hover(),
                    );
                    let split = row.left() + FILE_WIDTH;
                    let file = Rect::from_x_y_ranges(row.left()..=split, row.y_range());
                    let reason = Rect::from_x_y_ranges(split..=row.right(), row.y_range());
                    queue_cell(ui, file, |ui| {
                        ui.add(egui::Label::new(file_name(path)).truncate())
                            .on_hover_text(path_label(path));
                    });
                    queue_cell(ui, reason, |ui| self.show_error(ui, error));
                }
            });
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
            && let Some(selected) = rfd::FileDialog::new().pick_folder()
        {
            self.folder = Some(selected);
            self.folder_failure = None;
            ui.ctx().request_repaint();
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
                    ui.add_enabled(pending > 0, Button::new(self.label(22)))
                }
            })
            .clicked()
        {
            if let Some(worker) = &self.worker {
                worker.cancelled.store(true, Ordering::Relaxed);
            } else {
                self.start_conversion(ui.ctx());
            }
        }
        let busy = self.worker.is_some();
        if ui
            .put(center.translate(offset), |ui: &mut Ui| {
                ui.add_enabled(
                    !busy && (!self.queue.is_empty() || self.folder.is_some()),
                    Button::new(self.label(23)),
                )
            })
            .clicked()
        {
            self.queue.clear();
            self.folder = None;
            self.folder_failure = None;
            ui.ctx().request_repaint();
        }
        if let Some(error) = &self.folder_failure {
            self.show_error(ui, error);
        }
        if let Some(error) = &self.comparison_failure {
            self.show_error(ui, error);
        }
        if self.queue.is_empty() {
            if empty_drop_area(ui, &self.catalog)
                && let Some(files) = self.picker().pick_files()
            {
                self.add_files(files);
                ui.ctx().request_repaint();
            }
        } else {
            self.show_queue(ui, busy);
        }
    }

    fn start_conversion(&mut self, context: &Context) {
        self.folder_failure = None;
        let entries: Vec<_> = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.status, Status::Pending))
            .map(|(index, entry)| (index, entry.path.clone()))
            .collect();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.worker = Some(Batch {
            remaining: entries.len(),
            recent: VecDeque::new(),
            events: jobs::convert(
                context,
                entries,
                self.folder.clone(),
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
        let output_end = table.width() * 0.9;
        let min_width = MIN_COLUMN_WIDTH.min(output_end * 0.5);
        let file_width =
            (table.width() * self.file_column_fraction).clamp(min_width, output_end - min_width);
        let split = table.left() + file_width;
        let output_end_x = table.left() + output_end;
        let preview_end = table.left() + table.width() * 0.95;
        let column_rects = |row: Rect| {
            [
                Rect::from_x_y_ranges(row.left()..=split, row.y_range()),
                Rect::from_x_y_ranges(split..=output_end_x, row.y_range()),
                Rect::from_x_y_ranges(output_end_x..=preview_end, row.y_range()),
                Rect::from_x_y_ranges(preview_end..=table.right(), row.y_range()),
            ]
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
        let mut folder_failure = None;
        let scroll_bounds = Rect::from_min_max(body.min, bounds.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(scroll_bounds), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.spacing_mut().scroll = scroll_style;
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(body.height())
                .min_scrolled_height(0.0)
                .scroll_bar_visibility(if scrolls {
                    ScrollBarVisibility::AlwaysVisible
                } else {
                    ScrollBarVisibility::AlwaysHidden
                })
                .show_rows(ui, ROW_HEIGHT, self.queue.len(), |ui, rows| {
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
                            let [file, output, preview, delete] = column_rects(row);
                            queue_cell(ui, file, |ui| {
                                let show_name = |ui: &mut Ui| {
                                    ui.add(egui::Label::new(file_name(&entry.path)).truncate())
                                        .on_hover_text(path_label(&entry.path));
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
                            queue_cell(ui, output, |ui| {
                                self.show_output(ui, &entry.status, &mut folder_failure);
                            });
                            let conversion = match &entry.status {
                                Status::Done(Ok(Some(result))) => Some(result),
                                _ => None,
                            };
                            let compare_rect =
                                Rect::from_center_size(preview.center(), egui::vec2(28.0, 28.0));
                            if ui
                                .put(compare_rect, |ui: &mut Ui| {
                                    icons::compare(ui, self.label(122), conversion.is_some())
                                })
                                .clicked()
                                && let Some(conversion) = conversion
                            {
                                compare = Some([entry.path.clone(), conversion.path.clone()]);
                            }
                            let remove_rect =
                                Rect::from_center_size(delete.center(), egui::vec2(28.0, 28.0));
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
                });
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
                ui.id().with("queue_column_divider"),
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
        if folder_failure.is_some() {
            self.folder_failure = folder_failure;
        }
    }

    fn show_output(&self, ui: &mut Ui, status: &Status, folder_failure: &mut Option<Failure>) {
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
                            self.catalog.file_size(result.source_bytes),
                            self.catalog.file_size(result.bytes),
                        ))
                        .truncate()
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text(path_label(&result.path))
                    .context_menu(|ui| {
                        if ui.button(self.label(70)).clicked() {
                            if let Some(parent) = result.path.parent()
                                && let Err(error) = Command::new("xdg-open").arg(parent).spawn()
                            {
                                *folder_failure = Some(Failure {
                                    label: 71,
                                    detail: error.to_string(),
                                });
                            }
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
        while let Some(worker) = &mut self.worker {
            match worker.events.try_recv() {
                Ok(Event::Started(index)) => self.queue[index].status = Status::Running,
                Ok(Event::Converted(index, result, elapsed)) => {
                    self.queue[index].status = Status::Done(result);
                    worker.record(elapsed);
                }
                Ok(Event::Interrupted(index)) => self.queue[index].status = Status::Pending,
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

    fn open_comparison(&mut self, paths: &[PathBuf]) {
        self.comparison_failure = None;
        let child = std::env::current_exe().and_then(|executable| {
            Command::new(executable)
                .args(["--compare", &self.language, theme_name(self.theme)])
                .args(paths)
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
    fn update(&mut self, context: &Context, _: &mut eframe::Frame) {
        let size_change = context.input(|input| {
            let window = input.viewport();
            if window.fullscreen == Some(true) {
                Some(egui::ViewportCommand::Fullscreen(false))
            } else if window.maximized == Some(true) {
                Some(egui::ViewportCommand::Maximized(false))
            } else if window
                .inner_rect
                .is_some_and(|rect| (rect.size() - MAIN_WINDOW_SIZE).abs().max_elem() > 1.0)
            {
                Some(egui::ViewportCommand::InnerSize(MAIN_WINDOW_SIZE))
            } else {
                None
            }
        });
        if let Some(command) = size_change {
            context.send_viewport_cmd(command);
        }
        self.system_theme.apply(context, self.theme);
        self.poll_worker();
        self.add_files(self.file_drops.take(context));
        self.header(context);
        if !self.queue.is_empty() {
            self.footer(context);
        }
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

fn empty_drop_area(ui: &mut Ui, catalog: &Catalog) -> bool {
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
            ui.label(catalog.text(11));
            add_images_button(ui, catalog, true).clicked()
        },
    )
    .inner
}

fn add_images_button(ui: &mut Ui, catalog: &Catalog, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        Button::new(RichText::new("+").size(20.0)).min_size(ADD_IMAGES_SIZE),
    );
    let label = catalog.text(8);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
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

    #[test]
    fn estimate_uses_up_to_ten_completed_files_from_this_batch() {
        let (_, events) = std::sync::mpsc::channel();
        let mut batch = Batch {
            events,
            cancelled: Arc::new(AtomicBool::new(false)),
            remaining: 12,
            recent: VecDeque::new(),
        };
        assert_eq!(batch.estimate_seconds(), None);
        batch.record(Duration::from_secs(40));
        assert_eq!(batch.estimate_seconds(), Some(440));

        for _ in 0..10 {
            batch.record(Duration::from_secs(2));
        }
        assert_eq!(batch.estimate_seconds(), Some(2));
        batch.record(Duration::from_secs(2));
        assert_eq!(batch.estimate_seconds(), None);
    }
}
