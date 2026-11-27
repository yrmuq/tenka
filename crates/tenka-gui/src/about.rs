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
    updates::{self, ReleaseStatus},
    window_size::WindowSize,
};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const LICENSE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/LICENSE");
const LOGO_SIZE: egui::Vec2 = egui::vec2(88.0, 88.0);
const COLUMN_SPACING: f32 = 18.0;
const MIN_COLUMN_WIDTH: f32 = 120.0;

#[derive(Clone, Copy, Default)]
struct TableLayout {
    width: f32,
    upstream: f32,
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
            ui.horizontal(|ui| {
                ui.heading(catalog.text(153));
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
            let upstream = layout.upstream.max(MIN_COLUMN_WIDTH + COLUMN_SPACING);
            let title_left = table_left + upstream;
            let mut title_font = egui::TextStyle::Heading.resolve(ui.style());
            title_font.size *= 2.0;
            let title =
                ui.painter()
                    .layout_no_wrap("Tenka".into(), title_font, ui.visuals().text_color());
            let spacing = ui.spacing().item_spacing;
            let project_left =
                (area.center().x + spacing.x * 0.5).max(title_left + title.size().x + spacing.x);
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let project_top = area.top() + (logo.anchor.y - 1.5 * row_height - spacing.y).max(0.0);
            let project_area =
                egui::Rect::from_min_max(egui::pos2(project_left, project_top), area.max);
            let (check_for_updates, license_y) = ui
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
                    show_update_status(ui, catalog, checking, self.status.as_ref());
                    (check.inner, license.response.rect.center().y)
                })
                .inner;
            let title_rect = egui::Rect::from_min_size(
                egui::pos2(title_left, license_y - title.size().y * 0.5),
                title.size(),
            );
            ui.put(title_rect, egui::Label::new(title));
            // The striped rows extend two points beyond the first column.
            let logo_rect = egui::Rect::from_min_size(
                egui::pos2(table_left - 2.0, license_y) - logo.anchor,
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
            ui.add_space(18.0);
            let measured = show_components(ui, catalog, inset);
            (check_for_updates, logo_clicked, measured)
        });
        let (check_for_updates, logo_clicked, measured) = response.inner;
        if (layout.width - measured.width).abs() > 0.5
            || (layout.upstream - measured.upstream).abs() > 0.5
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
            ui.label(catalog.text(163));
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
    let grid = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(inset);
        egui::Grid::new("about_components")
            .num_columns(3)
            .min_col_width(MIN_COLUMN_WIDTH)
            .max_col_width(350.0)
            .spacing(egui::vec2(COLUMN_SPACING, 9.0))
            .striped(true)
            .show(ui, |ui| {
                let first = ui.strong(catalog.text(166));
                let upstream = ui.strong(catalog.text(167));
                ui.strong(catalog.text(168));
                ui.end_row();
                for component in components() {
                    ui.label(component.name);
                    ui.hyperlink_to(component.upstream, component.url);
                    ui.label(catalog.translate(component.details));
                    ui.end_row();
                }
                upstream.rect.left() - first.rect.left()
            })
    });
    TableLayout {
        width: grid.inner.response.rect.width(),
        upstream: grid.inner.inner,
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
    fn the_logo_and_project_details_align_with_the_centered_table() {
        let context = Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::MAIN_WINDOW_SIZE);
        let catalog = Catalog::new("en");
        let mut window = WindowSize::new();
        let mut about = About::default();
        about.open();
        let mut render = || {
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
            render();
        }
        let output = render();
        let stripe = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.width() > 360.0 && (15.0..50.0).contains(&rect.rect.height()) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            (stripe.center().x - screen.center().x).abs() <= 1.0,
            "{stripe:?}"
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
        assert!((logo_anchor.x - stripe.left()).abs() <= 1.0);
        let title = text_rect("Tenka");
        assert!((title.left() - text_rect("Upstream").left()).abs() <= 1.0);
        let license = text_rect("License");
        assert!((license.center().y - logo_anchor.y).abs() <= 1.0);
        for label in ["Repository", "License", "Version"] {
            let label_rect = text_rect(label);
            assert!(label_rect.left() > title.right());
            assert!((label_rect.left() - license.left()).abs() <= 1.0);
        }
    }
}
