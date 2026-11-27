use eframe::egui::{self, Context, ViewportCommand};

use crate::{icons, language::Catalog};

const DEFAULT_WIDTH: u16 = 760;
const DEFAULT_HEIGHT: u16 = 620;
const MIN_WIDTH: u16 = 760;
const MIN_HEIGHT: u16 = 480;
const MAX_DIMENSION: u16 = 8192;
pub const DEFAULT_SIZE: egui::Vec2 = egui::vec2(DEFAULT_WIDTH as f32, DEFAULT_HEIGHT as f32);
pub const MIN_SIZE: egui::Vec2 = egui::vec2(MIN_WIDTH as f32, MIN_HEIGHT as f32);

#[derive(Clone, Copy, PartialEq)]
struct Settings {
    resizable: bool,
    width: u16,
    height: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resizable: true,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }
}

impl Settings {
    fn size(self) -> egui::Vec2 {
        egui::vec2(f32::from(self.width), f32::from(self.height))
    }

    fn clamp_size(&mut self) {
        self.width = self.width.clamp(MIN_WIDTH, MAX_DIMENSION);
        self.height = self.height.clamp(MIN_HEIGHT, MAX_DIMENSION);
    }
}

pub struct WindowSize {
    settings: Settings,
    last_inner_size: Option<egui::Vec2>,
    resize_pending: bool,
    starting: bool,
}

impl WindowSize {
    pub fn new() -> Self {
        Self {
            settings: Settings::default(),
            last_inner_size: None,
            resize_pending: false,
            starting: true,
        }
    }

    fn apply(&mut self, context: &Context) {
        let Settings { resizable, .. } = self.settings;
        let size = self.settings.size();
        let minimum = if resizable { MIN_SIZE } else { size };
        let maximum = if resizable {
            egui::Vec2::INFINITY
        } else {
            size
        };
        // Apply the lock after Winit creates its Wayland decoration frame.
        context.send_viewport_cmd(ViewportCommand::Resizable(resizable));
        context.send_viewport_cmd(ViewportCommand::MinInnerSize(minimum));
        context.send_viewport_cmd(ViewportCommand::MaxInnerSize(maximum));
        context.send_viewport_cmd(ViewportCommand::EnableButtons {
            close: true,
            minimized: true,
            maximize: resizable,
        });
        self.resize_pending = true;
    }

    pub fn update(&mut self, context: &Context) {
        if std::mem::take(&mut self.starting) {
            self.apply(context);
            // The first show can still restore geometry from an older build.
            context.request_repaint();
            return;
        }
        let (fullscreen, maximized, inner_size) = context.input(|input| {
            let viewport = input.viewport();
            (
                viewport.fullscreen == Some(true),
                viewport.maximized == Some(true),
                viewport.inner_rect.map(|rect| rect.size()),
            )
        });
        if fullscreen || maximized {
            if self.resize_pending || !self.settings.resizable {
                let command = if fullscreen {
                    ViewportCommand::Fullscreen(false)
                } else {
                    ViewportCommand::Maximized(false)
                };
                context.send_viewport_cmd(command);
            }
            return;
        }

        let previous_size = self.last_inner_size;
        self.last_inner_size = inner_size;
        if self.resize_pending || !self.settings.resizable {
            let size = self.settings.size();
            if self.resize_pending
                || inner_size.is_some_and(|actual| (actual - size).abs().max_elem() > 1.0)
            {
                context.send_viewport_cmd(ViewportCommand::InnerSize(size));
            }
            self.resize_pending = false;
        } else if let (Some(previous), Some(actual)) = (previous_size, inner_size)
            && previous != actual
        {
            self.settings.width = actual.x.round() as u16;
            self.settings.height = actual.y.round() as u16;
            self.settings.clamp_size();
        }
    }

    pub fn show(&mut self, context: &Context, catalog: &Catalog) -> bool {
        let previous = self.settings;
        let dialog =
            egui::Modal::new(egui::Id::unique("tenka_window_settings")).show(context, |ui| {
                ui.set_width(310.0);
                ui.horizontal(|ui| {
                    ui.heading(catalog.text(178));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::close(ui, catalog.text(142)).clicked() {
                            ui.close_kind(egui::UiKind::Modal);
                        }
                    });
                });
                ui.checkbox(&mut self.settings.resizable, catalog.text(179));
                egui::Grid::new("tenka_window_dimensions").show(ui, |ui| {
                    ui.label(catalog.text(180));
                    ui.add(
                        egui::DragValue::new(&mut self.settings.width)
                            .range(MIN_WIDTH..=MAX_DIMENSION)
                            .update_while_editing(false),
                    );
                    ui.end_row();
                    ui.label(catalog.text(181));
                    ui.add(
                        egui::DragValue::new(&mut self.settings.height)
                            .range(MIN_HEIGHT..=MAX_DIMENSION)
                            .update_while_editing(false),
                    );
                    ui.end_row();
                });
                if ui.button(catalog.text(182)).clicked() {
                    self.settings = Settings::default();
                }
            });
        if previous != self.settings {
            self.apply(context);
            self.update(context);
        }
        dialog.response.should_close()
            || (dialog.is_top_modal
                && !dialog.any_popup_open
                && context
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
    }
}
