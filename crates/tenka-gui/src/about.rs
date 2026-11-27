use std::sync::{
    OnceLock,
    mpsc::{Receiver, TryRecvError},
};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, Context, TextureHandle, TextureOptions, Ui};

use crate::{
    APP_ICON, icons,
    jobs::Failure,
    language::Catalog,
    theme::CONVERT_ACCENT,
    updates::{self, ReleaseStatus},
    window_size::WindowSize,
};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/LICENSE");
const LOGO_SIZE: egui::Vec2 = egui::vec2(88.0, 88.0);
const COLUMN_SPACING: f32 = 18.0;
const ROW_SPACING: f32 = 9.0;
const MIN_COLUMN_WIDTH: f32 = 120.0;

#[derive(Clone, Copy, Default)]
struct TableLayout {
    width: f32,
    upstream: f32,
    details: f32,
}

struct Logo {
    texture: TextureHandle,
    anchor: egui::Vec2,
}

impl Logo {
    fn load(context: &Context) -> Self {
        let icon = eframe::icon_data::from_png_bytes(APP_ICON)
            .expect("the embedded Tenka icon is valid PNG");
        let size = [icon.width as usize, icon.height as usize];
        let pixels = ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
        let mut visible = egui::Rect::NOTHING;
        for (index, pixel) in pixels.pixels.iter().enumerate() {
            if pixel.a() > 0 {
                visible = visible.union(egui::Rect::from_min_size(
                    egui::pos2((index % size[0]) as f32, (index / size[0]) as f32),
                    egui::Vec2::splat(1.0),
                ));
            }
        }
        let scale = LOGO_SIZE / egui::vec2(icon.width as f32, icon.height as f32);
        let anchor = if visible.is_positive() {
            egui::vec2(visible.left(), visible.center().y) * scale
        } else {
            egui::vec2(0.0, LOGO_SIZE.y * 0.5)
        };
        Self {
            texture: context.load_texture("tenka_logo", pixels, TextureOptions::LINEAR),
            anchor,
        }
    }
}

#[derive(Default)]
struct LogoTaps {
    count: u8,
    last: Option<Instant>,
}

impl LogoTaps {
    fn push(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) > Duration::from_secs(2))
        {
            self.count = 0;
        }
        self.last = Some(now);
        self.count += 1;
        if self.count == 5 {
            *self = Self::default();
            return true;
        }
        false
    }
}

#[derive(Default)]
pub struct About {
    open: bool,
    window_settings_open: bool,
    logo_taps: LogoTaps,
    logo: Option<Logo>,
    table_layout: TableLayout,
    request: Option<Receiver<Result<ReleaseStatus, Failure>>>,
    status: Option<Result<ReleaseStatus, Failure>>,
}

impl About {
    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn show(&mut self, context: &Context, catalog: &Catalog, window: &mut WindowSize) {
        if let Some(request) = &self.request {
            let status = match request.try_recv() {
                Ok(status) => Some(status),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(Failure {
                    label: 176,
                    detail: String::new(),
                })),
            };
            if let Some(status) = status {
                self.request = None;
                self.status = Some(status);
            }
        }
        if !self.open {
            return;
        }
        if self.window_settings_open {
            if window.show(context, catalog) {
                self.window_settings_open = false;
                context.request_repaint();
            }
            return;
        }

        let logo = self.logo.get_or_insert_with(|| Logo::load(context));
        let checking = self.request.is_some();
        let layout = self.table_layout;
        let response = egui::Modal::new(egui::Id::unique("about_tenka")).show(context, |ui| {
            ui.set_width(688.0);
            ui.visuals_mut().hyperlink_color = if ui.visuals().dark_mode {
                CONVERT_ACCENT
            } else {
                egui::Color32::from_rgb(162, 74, 0)
            };
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::close(ui, catalog.text(142)).clicked() {
                        ui.close_kind(egui::UiKind::Modal);
                    }
                });
            });
            ui.add_space(12.0);
            let area = ui.available_rect_before_wrap();
            let inset = if layout.width > 0.0 {
                ((area.width() - layout.width) * 0.5).max(0.0)
            } else {
                0.0
            };
            let table_left = area.left() + inset;
            let column_stride = MIN_COLUMN_WIDTH + COLUMN_SPACING;
            let upstream = if layout.upstream > 0.0 {
                layout.upstream
            } else {
                column_stride
            };
            let details = if layout.details > 0.0 {
                layout.details
            } else {
                2.0 * column_stride
            };
            let title_left = table_left + upstream;
            let mut title_font = egui::TextStyle::Heading.resolve(ui.style());
            title_font.size *= 2.0;
            let title =
                ui.painter()
                    .layout_no_wrap("Tenka".into(), title_font, ui.visuals().text_color());
            let spacing = ui.spacing().item_spacing;
            let project_left = table_left + details;
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let line_height = ui.spacing().interact_size.y.max(row_height) + spacing.y;
            let project_top = area.top() + (logo.anchor.y - 1.5 * row_height - spacing.y).max(0.0);
            let project_area =
                egui::Rect::from_min_max(egui::pos2(project_left, project_top), area.max);
            let (check_for_updates, license_y, table_top) = ui
                .scope_builder(egui::UiBuilder::new().max_rect(project_area), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(catalog.text(154));
                        ui.hyperlink_to("GitHub", REPOSITORY);
                    });
                    let license = ui.horizontal(|ui| {
                        ui.label(catalog.text(155));
                        ui.hyperlink_to(env!("CARGO_PKG_LICENSE"), LICENSE);
                    });
                    let check = ui.horizontal_wrapped(|ui| {
                        ui.label(catalog.text(156));
                        ui.label(env!("CARGO_PKG_VERSION"));
                        ui.add_enabled(!checking, egui::Button::new(catalog.text(157)))
                            .clicked()
                    });
                    let table_top = ui.next_widget_position().y + 2.0 * line_height;
                    show_update_status(ui, catalog, checking, self.status.as_ref());
                    (check.inner, license.response.rect.center().y, table_top)
                })
                .inner;
            let title_rect = egui::Rect::from_min_size(
                egui::pos2(title_left, license_y - title.size().y * 0.5),
                title.size(),
            );
            ui.put(title_rect, egui::Label::new(title));
            let logo_rect = egui::Rect::from_min_size(
                egui::pos2(table_left, license_y) - logo.anchor,
                LOGO_SIZE,
            );
            let logo_clicked = ui
                .put(
                    logo_rect,
                    egui::Image::new(&logo.texture)
                        .fit_to_exact_size(LOGO_SIZE)
                        .sense(egui::Sense::click()),
                )
                .clicked();
            ui.add_space((table_top - ui.next_widget_position().y).max(0.0));
            let measured = show_components(ui, catalog, inset);
            ui.add_space(line_height);
            (check_for_updates, logo_clicked, measured)
        });
        let (check_for_updates, logo_clicked, measured) = response.inner;
        if (layout.width - measured.width).abs() > 0.5
            || (layout.upstream - measured.upstream).abs() > 0.5
            || (layout.details - measured.details).abs() > 0.5
        {
            self.table_layout = measured;
            context.request_repaint();
        }
        if check_for_updates {
            self.request = Some(updates::start(context));
        }
        if response.should_close() {
            self.open = false;
            self.logo_taps = LogoTaps::default();
        } else if logo_clicked && self.logo_taps.push(Instant::now()) {
            self.window_settings_open = true;
            context.request_repaint();
        }
    }
}

fn show_update_status(
    ui: &mut Ui,
    catalog: &Catalog,
    checking: bool,
    status: Option<&Result<ReleaseStatus, Failure>>,
) {
    if checking {
        ui.weak(catalog.text(158));
        return;
    }
    match status {
        None => {}
        Some(Ok(ReleaseStatus::Unpublished)) => {
            ui.weak(catalog.text(162));
        }
        Some(Ok(ReleaseStatus::Current)) => {
            let color = if ui.visuals().dark_mode {
                egui::Color32::LIGHT_GREEN
            } else {
                egui::Color32::DARK_GREEN
            };
            ui.colored_label(color, catalog.text(163));
        }
        Some(Ok(ReleaseStatus::Available(tag))) => {
            ui.label(format!("{} · {tag}", catalog.text(164)));
            ui.hyperlink_to(catalog.text(165), updates::RELEASE_PAGE);
        }
        Some(Err(error)) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("[E{}] {}", error.label, catalog.text(error.label)),
            )
            .on_hover_text(&error.detail);
        }
    }
}

struct Component {
    name: &'static str,
    upstream: &'static str,
    url: &'static str,
    details: &'static str,
}

fn components() -> &'static [Component] {
    static ROWS: OnceLock<Vec<Component>> = OnceLock::new();
    ROWS.get_or_init(|| {
        include_str!("../../../README.md")
            .lines()
            .skip_while(|line| *line != "## License & Attribution")
            .skip(1)
            .take_while(|line| !line.starts_with("## "))
            .filter_map(|line| {
                let mut cells = line.strip_prefix('|')?.strip_suffix('|')?.split('|');
                let name = cells.next()?.trim().trim_matches('`');
                let (upstream, url) = cells.next()?.trim().strip_prefix('[')?.split_once("](")?;
                let url = url.strip_suffix(')')?;
                let _license = cells.next()?;
                let details = cells.next()?.trim();
                url.starts_with("https://").then_some(Component {
                    name,
                    upstream,
                    url,
                    details,
                })
            })
            .collect()
    })
}

fn show_components(ui: &mut Ui, catalog: &Catalog, inset: f32) -> TableLayout {
    let mut row_dividers = Vec::with_capacity(components().len());
    let grid = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(inset);
        egui::Grid::new("about_components")
            .num_columns(3)
            .min_col_width(MIN_COLUMN_WIDTH)
            .max_col_width(350.0)
            .spacing(egui::vec2(COLUMN_SPACING, ROW_SPACING))
            .show(ui, |ui| {
                let first = ui.strong(catalog.text(166));
                let upstream = ui.strong(catalog.text(167));
                let details = ui.strong(catalog.text(168));
                ui.end_row();
                for component in components() {
                    row_dividers.push(ui.next_widget_position().y - ROW_SPACING * 0.5);
                    ui.label(component.name);
                    ui.hyperlink_to(component.upstream, component.url);
                    ui.label(catalog.translate(component.details));
                    ui.end_row();
                }
                (
                    upstream.rect.left() - first.rect.left(),
                    details.rect.left() - first.rect.left(),
                )
            })
    });
    let grid = grid.inner;
    let (upstream, details) = grid.inner;
    let body = grid.response.rect;
    let border = body.expand2(egui::vec2(COLUMN_SPACING, ROW_SPACING) * 0.5);
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .rect_stroke(border, 0.0, stroke, egui::StrokeKind::Inside);
    for column in [upstream, details] {
        ui.painter()
            .vline(border.left() + column, border.y_range(), stroke);
    }
    for row in row_dividers {
        ui.painter().hline(border.x_range(), row, stroke);
    }
    TableLayout {
        width: body.width(),
        upstream,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_uses_the_readme_links_and_translates_descriptions() {
        let rows = components();
        assert!(rows.iter().any(|row| {
            row.name == "libjxl"
                && row.upstream == "libjxl/libjxl"
                && row.url == "https://github.com/libjxl/libjxl"
                && row.details == "JPEG XL codec."
        }));
        let russian = Catalog::new("ru");
        assert_eq!(russian.translate("JPEG XL codec."), "Кодек JPEG XL.");
        assert!(rows.iter().any(|row| row.name == "SSIMULACRA2"));
    }

    #[test]
    fn the_about_header_aligns_with_the_table_and_leaves_room_for_status() {
        let context = Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::MAIN_WINDOW_SIZE);
        let catalog = Catalog::new("en");
        let mut window = WindowSize::new();
        let mut about = About::default();
        about.open();
        let mut render = |about: &mut About| {
            crate::tests::render_ui(
                &context,
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| about.show(ui.ctx(), &catalog, &mut window),
            )
        };
        for _ in 0..3 {
            render(&mut about);
        }
        let output = render(&mut about);
        let table_border = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.width() > 360.0
                        && rect.rect.height() > 160.0
                        && rect.fill == egui::Color32::TRANSPARENT
                        && !rect.stroke.is_empty() =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            (table_border.center().x - screen.center().x).abs() <= 1.0,
            "{table_border:?}"
        );
        let logo = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        let text_rect = |label| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        let logo_anchor = logo.min + about.logo.as_ref().unwrap().anchor;
        assert!((logo_anchor.x - table_border.left() - COLUMN_SPACING * 0.5).abs() <= 1.0);
        assert!((logo_anchor.x - text_rect(catalog.text(166)).left()).abs() <= 1.0);
        let title = text_rect("Tenka");
        assert!((title.left() - text_rect(catalog.text(167)).left()).abs() <= 1.0);
        let license = text_rect("License");
        assert!((license.center().y - logo_anchor.y).abs() <= 1.0);
        for label in ["Repository", "License", "Version"] {
            let label_rect = text_rect(label);
            assert!(label_rect.left() > title.right());
            assert!((label_rect.left() - text_rect(catalog.text(168)).left()).abs() <= 1.0);
        }
        let table_top = text_rect(catalog.text(166)).top();
        let version_bottom = text_rect(catalog.text(156)).bottom();
        about.status = Some(Ok(ReleaseStatus::Current));
        for (theme, color) in [
            (egui::Theme::Dark, egui::Color32::LIGHT_GREEN),
            (egui::Theme::Light, egui::Color32::DARK_GREEN),
        ] {
            context.set_theme(theme);
            let checked = render(&mut about);
            let text = |label| {
                checked.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                    _ => None,
                })
            };
            let status = text(catalog.text(163)).unwrap();
            assert_eq!(status.galley.job.sections[0].format.color, color);
            let style = context.style_of(theme);
            let spacing = style.spacing.item_spacing.y;
            let row_height = style.spacing.interact_size.y;
            let line_height = row_height + spacing;
            let header = text(catalog.text(166)).unwrap();
            let header_inset = (row_height - header.galley.rect.height()).max(0.0) * 0.5;
            assert!((status.pos.y - version_bottom - spacing).abs() <= 1.0);
            assert!((header.pos.y - table_top).abs() <= 1.0);
            let gap = table_top - header_inset - status.pos.y;
            assert!(
                (gap - 2.0 * line_height).abs() <= 1.0,
                "gap: {gap}, line height: {line_height}"
            );
        }
    }
}
