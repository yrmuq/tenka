use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError},
};

use eframe::egui::{
    self, Align2, Color32, Context, FontId, IconData, Label, PointerButton, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, TextureFilter, TextureHandle, TextureOptions, ThemePreference, Ui,
    Vec2, pos2, vec2,
};

use crate::{
    ADD_IMAGES_SIZE, add_images_button, empty_drop_area,
    file_drop::FileDrops,
    file_name,
    jobs::{self, ComparisonPixels, Failure},
    language::Catalog,
    path_label,
    theme::SystemTheme,
};

pub fn run(
    icon: IconData,
    language: &str,
    theme: ThemePreference,
    paths: Vec<PathBuf>,
) -> eframe::Result {
    let catalog = Catalog::new(language);
    let title = format!("Tenka — {}", catalog.text(99));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("io.github.yrmuq.tenka.compare")
            .with_icon(icon)
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([320.0, 240.0]),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_theme(theme);
            let mut comparison = Comparison {
                catalog,
                theme,
                system_theme: SystemTheme::new(&creation.egui_ctx),
                file_drops: FileDrops::new(&creation.egui_ctx),
                images: Vec::new(),
                next: 0,
                loading: None,
                camera: Camera::default(),
                split: true,
            };
            comparison.add(paths);
            Ok(Box::new(comparison))
        }),
    )
}

struct Comparison {
    catalog: Catalog,
    theme: ThemePreference,
    system_theme: SystemTheme,
    file_drops: FileDrops,
    images: Vec<Entry>,
    next: usize,
    loading: Option<Receiver<Result<ComparisonPixels, Failure>>>,
    camera: Camera,
    split: bool,
}

struct Entry {
    path: PathBuf,
    view: EntryView,
}

enum EntryView {
    Pending,
    Image {
        size: Vec2,
        tiles: Vec<(Rect, TextureHandle)>,
    },
    Failed(Failure),
}

impl Comparison {
    fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.images.extend(paths.into_iter().map(|path| Entry {
            path,
            view: EntryView::Pending,
        }));
    }

    fn pick_images(&mut self) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter(
                "JPEG / PNG / WebP / JPEG XL",
                &["jpg", "jpeg", "png", "webp", "jxl"],
            )
            .pick_files()
        {
            self.add(files);
        }
    }

    fn load_images(&mut self, context: &Context) {
        if let Some(receiver) = &self.loading {
            let entry = &mut self.images[self.next];
            entry.view = match receiver.try_recv() {
                Ok(Ok(image)) => {
                    let [width, height] = image.dimensions;
                    let tiles = image
                        .tiles
                        .into_iter()
                        .map(|(bounds, pixels)| {
                            let texture = context.load_texture(
                                file_name(&entry.path),
                                pixels,
                                TextureOptions {
                                    magnification: TextureFilter::Nearest,
                                    ..TextureOptions::LINEAR
                                },
                            );
                            (bounds, texture)
                        })
                        .collect();
                    EntryView::Image {
                        size: vec2(width as f32, height as f32),
                        tiles,
                    }
                }
                Ok(Err(error)) => EntryView::Failed(error),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => EntryView::Failed(Failure {
                    label: 61,
                    detail: String::new(),
                }),
            };
            self.next += 1;
            self.loading = None;
        }
        if let Some(entry) = self.images.get(self.next) {
            let max_texture_side = context.input(|input| input.max_texture_side).max(1);
            self.loading = Some(jobs::load_comparison(
                context,
                entry.path.clone(),
                max_texture_side,
            ));
        }
    }

    fn show_images(&mut self, ui: &mut Ui) {
        let area = ui.max_rect();
        if self.images.is_empty() {
            let drop_area = area.shrink2(vec2(20.0, 15.0));
            if ui
                .scope_builder(egui::UiBuilder::new().max_rect(drop_area), |ui| {
                    empty_drop_area(ui, &self.catalog, true)
                })
                .inner
            {
                self.pick_images();
                self.load_images(ui.ctx());
            }
            return;
        }

        let shared_frame = if self.split {
            self.images.iter().find_map(|entry| match &entry.view {
                EntryView::Image { size, .. } => Some((area, *size)),
                _ => None,
            })
        } else {
            None
        };
        let regions = cells(area, self.images.len())
            .into_iter()
            .map(|cell| if self.split { cell } else { cell.shrink(1.0) })
            .collect::<Vec<_>>();

        for (index, (entry, &region)) in self.images.iter().zip(&regions).enumerate() {
            let EntryView::Image { size, .. } = &entry.view else {
                continue;
            };
            let (frame, size) = shared_frame.unwrap_or((region, *size));
            let response = ui.interact(region, ui.id().with(index), Sense::drag());
            let fitted = fit(size, frame.size());
            if response.dragged_by(PointerButton::Primary) {
                self.camera.pan(response.drag_delta(), fitted);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if let Some(pointer) = response.hover_pos() {
                let scroll = ui.input_mut(|input| std::mem::take(&mut input.raw_scroll_delta.y));
                if scroll != 0.0 {
                    self.camera
                        .zoom_at(scroll, pointer - frame.center(), fitted);
                }
            }
        }

        for (index, (entry, &region)) in self.images.iter().zip(&regions).enumerate() {
            let painter = ui.painter().with_clip_rect(region);
            painter.rect_filled(region, 0, ui.visuals().extreme_bg_color);
            match &entry.view {
                EntryView::Image { size, tiles } => {
                    let (frame, size) = shared_frame.unwrap_or((region, *size));
                    let image = self.camera.image_rect(frame, size);
                    for (bounds, texture) in tiles {
                        let tile = Rect::from_min_max(
                            image.lerp_inside(bounds.min.to_vec2()),
                            image.lerp_inside(bounds.max.to_vec2()),
                        );
                        if tile.intersects(region) {
                            painter.image(
                                texture.id(),
                                tile,
                                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                    }
                }
                EntryView::Pending => {
                    egui::Spinner::new().paint_at(
                        ui,
                        Rect::from_center_size(region.center(), Vec2::splat(18.0)),
                    );
                }
                EntryView::Failed(error) => {
                    painter.text(
                        region.center(),
                        Align2::CENTER_CENTER,
                        format!("[E{}] {}", error.label, self.catalog.text(error.label)),
                        FontId::proportional(14.0),
                        ui.visuals().error_fg_color,
                    );
                    ui.interact(region, ui.id().with(index), Sense::hover())
                        .on_hover_text(&error.detail);
                }
            }
        }

        if self.split && regions.len() > 1 {
            let painter = ui.painter_at(area);
            for stroke in [
                Stroke::new(3.0_f32, Color32::BLACK),
                Stroke::new(1.0_f32, Color32::WHITE),
            ] {
                for region in &regions {
                    if region.left() > area.left() {
                        painter.vline(region.left(), region.y_range(), stroke);
                    }
                    if region.top() > area.top() {
                        painter.hline(region.x_range(), region.top(), stroke);
                    }
                }
            }
        }
    }

    fn show_captions(&self, context: &Context) {
        let count = self.images.len();
        if count == 0 {
            return;
        }
        let height = row_count(count) as f32 * 38.0;
        let panel_height = height.min((context.available_rect().height() / 2.0).max(38.0));
        egui::TopBottomPanel::bottom("comparison_captions")
            .exact_height(panel_height)
            .frame(egui::Frame::side_top_panel(&context.style()).inner_margin(0))
            .show(context, |ui| {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let (area, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), height),
                            Sense::hover(),
                        );
                        for (entry, cell) in self.images.iter().zip(cells(area, count)) {
                            ui.put(
                                cell.shrink2(vec2(6.0, 0.0)),
                                Label::new(RichText::new(file_name(&entry.path)).size(26.0))
                                    .truncate(),
                            )
                            .on_hover_text(path_label(&entry.path));
                        }
                    });
            });
    }
}

impl eframe::App for Comparison {
    fn update(&mut self, context: &Context, _: &mut eframe::Frame) {
        self.system_theme.apply(context, self.theme);
        self.add(self.file_drops.take(context));
        egui::TopBottomPanel::top("comparison_controls").show(context, |ui| {
            let (controls, _) = ui.allocate_exact_size(
                vec2(ui.available_width(), ADD_IMAGES_SIZE.y),
                Sense::hover(),
            );
            let mode = Rect::from_center_size(controls.center(), vec2(76.0, ADD_IMAGES_SIZE.y));
            if ui
                .put(mode, egui::Button::new(self.catalog.text(110)))
                .on_hover_text(self.catalog.text(if self.split { 104 } else { 123 }))
                .clicked()
            {
                self.split = !self.split;
            }
            if !self.images.is_empty()
                && ui
                    .put(
                        Rect::from_min_size(controls.min, ADD_IMAGES_SIZE),
                        |ui: &mut Ui| add_images_button(ui, &self.catalog, true),
                    )
                    .clicked()
            {
                self.pick_images();
            }
        });
        self.load_images(context);
        self.show_captions(context);
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&context.style()).inner_margin(0))
            .show(context, |ui| self.show_images(ui));
    }
}

fn cells(area: Rect, count: usize) -> Vec<Rect> {
    let rows = row_count(count);
    let height = area.height() / rows as f32;
    let mut cells = Vec::with_capacity(count);
    for row in 0..rows {
        let count_in_row = count / rows + usize::from(row < count % rows);
        let width = area.width() / count_in_row as f32;
        for column in 0..count_in_row {
            let top_left = area.min + vec2(column as f32 * width, row as f32 * height);
            cells.push(Rect::from_min_size(top_left, vec2(width, height)));
        }
    }
    cells
}

fn row_count(count: usize) -> usize {
    let columns = count.div_ceil(count.isqrt());
    count.div_ceil(columns)
}

fn fit(image: Vec2, area: Vec2) -> Vec2 {
    image * (area.x / image.x).min(area.y / image.y)
}

struct Camera {
    center: Vec2,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Vec2::splat(0.5),
            zoom: 1.0,
        }
    }
}

impl Camera {
    fn image_rect(&self, area: Rect, size: Vec2) -> Rect {
        let span = fit(size, area.size()) * self.zoom;
        Rect::from_min_size(area.center() - self.center * span, span)
    }

    fn zoom_at(&mut self, scroll: f32, pointer_offset: Vec2, fitted: Vec2) {
        let factor = (scroll * 0.01).clamp(-1.0, 1.0).exp();
        let zoom = (self.zoom * factor).clamp(1.0, 1024.0);
        let anchor = self.center + pointer_offset / (fitted * self.zoom);
        self.center =
            (anchor - pointer_offset / (fitted * zoom)).clamp(Vec2::ZERO, Vec2::splat(1.0));
        self.zoom = zoom;
    }

    fn pan(&mut self, delta: Vec2, fitted: Vec2) {
        self.center =
            (self.center - delta / (fitted * self.zoom)).clamp(Vec2::ZERO, Vec2::splat(1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_and_pan_matches_different_resolutions() {
        let mut camera = Camera::default();
        let first = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
        let second = first.translate(vec2(400.0, 0.0));
        let original = vec2(1600.0, 900.0);
        let resized = vec2(800.0, 450.0);
        let anchor = vec2(0.7, 0.3);
        let pointer = camera.image_rect(first, original).lerp_inside(anchor);
        let other_pointer = camera.image_rect(second, resized).lerp_inside(anchor);
        let fit = fit(original, first.size());

        camera.zoom_at(40.0, pointer - first.center(), fit);
        assert!(
            camera
                .image_rect(first, original)
                .lerp_inside(anchor)
                .distance(pointer)
                < 0.01
        );
        assert!(
            camera
                .image_rect(second, resized)
                .lerp_inside(anchor)
                .distance(other_pointer)
                < 0.01
        );
        let drag = vec2(-20.0, 10.0);
        camera.pan(drag, fit);
        assert!(
            camera
                .image_rect(first, original)
                .lerp_inside(anchor)
                .distance(pointer + drag)
                < 0.01
        );
        assert!(
            camera
                .image_rect(second, resized)
                .lerp_inside(anchor)
                .distance(other_pointer + drag)
                < 0.01
        );
    }
}
