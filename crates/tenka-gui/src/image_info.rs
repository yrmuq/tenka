use std::{
    fs::{self, Metadata},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui::{self, ColorImage, Context, TextureHandle, TextureOptions, Ui};
use tenka_core::{ImageDetails, MAX_INPUT_BYTES};

use crate::{
    file_name, icons,
    jobs::{self, Failure},
    language::Catalog,
    path_label,
};

struct FileDetails {
    metadata: Metadata,
    image: Result<ImageDetails, Failure>,
}

type DetailsResult = Result<FileDetails, Failure>;

#[derive(Default)]
pub struct ImageInfo {
    path: Option<PathBuf>,
    pending: Option<(PathBuf, Receiver<DetailsResult>)>,
    details: Option<DetailsResult>,
    preview: Option<TextureHandle>,
    viewer_request: Option<Receiver<Result<(), Failure>>>,
    viewer_failure: Option<Failure>,
}

impl ImageInfo {
    pub fn open(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.details = None;
        self.preview = None;
        self.viewer_request = None;
        self.viewer_failure = None;
    }

    fn poll(&mut self, context: &Context) {
        let Some((path, receiver)) = &self.pending else {
            return;
        };
        let mut result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(Failure {
                label: 196,
                detail: String::new(),
            }),
        };
        if self.path.as_ref() == Some(path) {
            if let Ok(details) = &mut result
                && let Ok(image) = &mut details.image
                && let Some(preview) = image.preview.take()
            {
                let size = [preview.width() as usize, preview.height() as usize];
                self.preview = Some(context.load_texture(
                    "image_info_preview",
                    ColorImage::from_rgba_unmultiplied(size, preview.as_raw()),
                    TextureOptions::LINEAR,
                ));
            }
            self.details = Some(result);
        }
        self.pending = None;
    }

    pub fn show(&mut self, context: &Context, catalog: &Catalog) {
        self.poll(context);
        if let Some(request) = &self.viewer_request {
            let result = match request.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(Failure {
                    label: 200,
                    detail: String::new(),
                })),
            };
            if let Some(result) = result {
                self.viewer_failure = result.err();
                self.viewer_request = None;
            }
        }
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if self.pending.is_none() && self.details.is_none() {
            self.pending = Some((path.to_owned(), load_details(context, path.to_owned())));
        }

        let available_height = context.content_rect().height() - 40.0;
        let dialog = egui::Modal::new(egui::Id::unique("image_details")).show(context, |ui| {
            ui.set_width(470.0);
            let top = ui.next_widget_position().y;
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::close(ui, catalog.text(142)).clicked() {
                        ui.close_kind(egui::UiKind::Modal);
                    }
                });
            });
            let mut open_viewer = false;
            match &self.details {
                Some(Ok(details)) => {
                    open_viewer = show_properties(
                        ui,
                        catalog,
                        path,
                        details,
                        self.preview.as_ref(),
                        (available_height - 265.0).clamp(60.0, 225.0),
                    );
                    if let Some(error) = &self.viewer_failure {
                        show_error(ui, catalog, error);
                    }
                    ui.separator();
                    let exif_height =
                        (available_height - (ui.next_widget_position().y - top) - 30.0).max(48.0);
                    egui::CollapsingHeader::new(catalog.text(191))
                        .id_salt(path)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(exif_height)
                                .show(ui, |ui| {
                                    show_exif(ui, catalog, details.image.as_ref().ok());
                                });
                        });
                }
                Some(Err(error)) => show_error(ui, catalog, error),
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(catalog.text(195));
                    });
                }
            }
            open_viewer
        });
        if dialog.inner {
            self.viewer_request = Some(jobs::open_image(context, path.to_owned()));
            self.viewer_failure = None;
        }
        if dialog.should_close() {
            self.path = None;
            self.details = None;
            self.preview = None;
            self.viewer_request = None;
            self.viewer_failure = None;
        }
    }
}

fn load_details(context: &Context, path: PathBuf) -> Receiver<DetailsResult> {
    let context = context.clone();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = fs::metadata(&path)
            .map_err(|error| Failure {
                label: 59,
                detail: error.to_string(),
            })
            .map(|metadata| {
                let image = if metadata.len() > MAX_INPUT_BYTES {
                    Err(Failure::from(tenka_core::Error::Limits))
                } else {
                    jobs::read_image(&path)
                        .and_then(|bytes| tenka_core::inspect_image(&bytes).map_err(Failure::from))
                };
                FileDetails { metadata, image }
            });
        if send.send(result).is_ok() {
            context.request_repaint();
        }
    });
    receive
}

fn show_properties(
    ui: &mut Ui,
    catalog: &Catalog,
    path: &Path,
    details: &FileDetails,
    preview: Option<&TextureHandle>,
    preview_height: f32,
) -> bool {
    let open_viewer = ui
        .vertical_centered(|ui| {
            if let Some(preview) = preview {
                ui.add(
                    egui::Image::new(preview)
                        .max_height(preview_height)
                        .max_width(410.0)
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(catalog.text(199))
                .double_clicked()
            } else {
                ui.weak(catalog.text(193));
                false
            }
        })
        .inner;
    ui.separator();

    let extension = path.extension().map(|value| value.to_string_lossy());
    let image = details.image.as_ref().ok();
    let size = catalog.file_size(details.metadata.len());
    let dimensions = image
        .and_then(|image| image.dimensions)
        .map(|(width, height)| format!("{width} × {height}"));
    let format = image.and_then(|image| image.format.as_deref());
    let created = details.metadata.created().ok().and_then(file_date);
    let modified = details.metadata.modified().ok().and_then(file_date);
    egui::Grid::new("image_properties")
        .num_columns(2)
        .max_col_width(320.0)
        .show(ui, |ui| {
            ui.strong(catalog.text(185));
            ui.add(egui::Label::new(file_name(path)).truncate())
                .on_hover_text(path_label(path));
            ui.end_row();
            for (label, value) in [
                (186, Some(size)),
                (187, dimensions),
                (188, format.map(str::to_owned)),
                (197, extension.as_ref().map(|value| format!(".{value}"))),
                (189, created),
                (190, modified),
            ] {
                ui.strong(catalog.text(label));
                ui.label(value.as_deref().unwrap_or("—"));
                ui.end_row();
            }
        });
    if let Some(extension) = extension.as_deref()
        && let Some(image) = image
        && !image.extensions.is_empty()
        && !image
            .extensions
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    {
        ui.colored_label(ui.visuals().warn_fg_color, catalog.text(192));
    }
    if let Err(error) = &details.image {
        show_error(ui, catalog, error);
    }
    open_viewer
}

fn show_exif(ui: &mut Ui, catalog: &Catalog, image: Option<&ImageDetails>) {
    if let Some(image) = image.filter(|image| !image.exif.is_empty()) {
        egui::Grid::new("image_exif")
            .num_columns(2)
            .max_col_width(285.0)
            .striped(true)
            .show(ui, |ui| {
                for (tag, value) in &image.exif {
                    ui.label(tag);
                    ui.label(value);
                    ui.end_row();
                }
            });
    } else {
        ui.weak(catalog.text(194));
    }
}

fn file_date(date: SystemTime) -> Option<String> {
    let nanos = match date.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    let date = time::OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        date.year(),
        date.month() as u8,
        date.day(),
        date.hour(),
        date.minute(),
        date.second(),
    ))
}

fn show_error(ui: &mut Ui, catalog: &Catalog, error: &Failure) {
    ui.colored_label(
        ui.visuals().error_fg_color,
        format!("[E{}] {}", error.label, catalog.text(error.label)),
    )
    .on_hover_text(&error.detail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_the_image_requires_a_double_click_on_the_preview() {
        let context = Context::default();
        let catalog = Catalog::new("en");
        let file = tempfile::NamedTempFile::new().unwrap();
        let details = FileDetails {
            metadata: file.as_file().metadata().unwrap(),
            image: Ok(ImageDetails {
                format: Some("PNG".into()),
                extensions: &["png"],
                dimensions: Some((128, 128)),
                preview: None,
                exif: Vec::new(),
            }),
        };
        let preview = context.load_texture(
            "test_double_click",
            ColorImage::filled([128, 128], egui::Color32::WHITE),
            TextureOptions::LINEAR,
        );
        let render = |time, events| {
            let mut open = false;
            let output = crate::tests::render_ui(
                &context,
                egui::RawInput {
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    open =
                        show_properties(ui, &catalog, file.path(), &details, Some(&preview), 128.0);
                },
            );
            (open, output)
        };
        let (_, output) = render(0.0, Vec::new());
        let preview_center = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect.center()),
                _ => None,
            })
            .unwrap();
        assert!(!render(0.1, crate::tests::click(preview_center)).0);
        assert!(render(0.2, crate::tests::click(preview_center)).0);
    }

    #[test]
    fn preview_and_last_file_date_are_visible_before_expanding_exif() {
        let context = Context::default();
        let catalog = Catalog::new("en");
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut card = ImageInfo {
            path: Some(file.path().to_owned()),
            details: Some(Ok(FileDetails {
                metadata: file.as_file().metadata().unwrap(),
                image: Ok(ImageDetails {
                    format: Some("JPEG".into()),
                    extensions: &["jpg", "jpeg"],
                    dimensions: Some((384, 384)),
                    preview: None,
                    exif: vec![("Camera model".into(), "Example camera".into())],
                }),
            })),
            preview: Some(context.load_texture(
                "test_preview",
                ColorImage::filled([384, 384], egui::Color32::WHITE),
                TextureOptions::LINEAR,
            )),
            ..Default::default()
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::MAIN_WINDOW_SIZE);
        let mut render = || {
            crate::tests::render_ui(
                &context,
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| card.show(ui.ctx(), &catalog),
            )
        };
        render();
        let output = render();
        let visible = |label: &str| {
            output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    let center = text.pos + text.galley.rect.center().to_vec2();
                    shape.clip_rect.contains(center) && screen.contains(center)
                }
                _ => false,
            })
        };
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::Shape::Rect(rect) if rect.brush.is_some()
                && rect.rect.height() >= 200.0 && screen.contains_rect(rect.rect)
        )));
        assert!(visible("Created"));
        assert!(visible("Modified"));
        assert!(visible("EXIF data"));
        assert!(!visible("Image information"));
        assert!(!visible("Camera model"));
    }
}
