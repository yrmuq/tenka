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
    logo: Option<TextureHandle>,
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

        let logo = self.logo.get_or_insert_with(|| {
            let icon = eframe::icon_data::from_png_bytes(APP_ICON)
                .expect("the embedded Tenka icon is valid PNG");
            let pixels = ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            context.load_texture("tenka_logo", pixels, TextureOptions::LINEAR)
        });
        let checking = self.request.is_some();
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
            let mut check_for_updates = false;
            let mut logo_clicked = false;
            ui.columns(2, |columns| {
                columns[0].vertical_centered(|ui| {
                    logo_clicked = ui
                        .add(
                            egui::Image::new(&*logo)
                                .fit_to_exact_size(egui::vec2(88.0, 88.0))
                                .sense(egui::Sense::click()),
                        )
                        .clicked();
                    ui.heading("Tenka");
                });
                columns[1].horizontal(|ui| {
                    ui.label(catalog.text(154));
                    ui.hyperlink_to("GitHub", REPOSITORY);
                });
                columns[1].horizontal(|ui| {
                    ui.label(catalog.text(155));
                    ui.hyperlink_to(env!("CARGO_PKG_LICENSE"), LICENSE);
                });
                columns[1].horizontal(|ui| {
                    ui.label(catalog.text(156));
                    ui.label(env!("CARGO_PKG_VERSION"));
                    check_for_updates = ui
                        .add_enabled(!checking, egui::Button::new(catalog.text(157)))
                        .clicked();
                });
                show_update_status(&mut columns[1], catalog, checking, self.status.as_ref());
            });
            ui.add_space(18.0);
            show_components(ui, catalog);
            (check_for_updates, logo_clicked)
        });
        let (check_for_updates, logo_clicked) = response.inner;
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

fn show_components(ui: &mut Ui, catalog: &Catalog) {
    egui::Grid::new("about_components")
        .num_columns(3)
        .min_col_width(120.0)
        .max_col_width(350.0)
        .spacing(egui::vec2(18.0, 9.0))
        .striped(true)
        .show(ui, |ui| {
            for id in [166, 167, 168] {
                ui.strong(catalog.text(id));
            }
            ui.end_row();
            for component in components() {
                ui.label(component.name);
                ui.hyperlink_to(component.upstream, component.url);
                ui.label(catalog.translate(component.details));
                ui.end_row();
            }
        });
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
}
