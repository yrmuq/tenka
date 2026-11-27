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
}

impl ImageInfo {
    pub fn open(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.details = None;
        self.preview = None;
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
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if self.pending.is_none() && self.details.is_none() {
            self.pending = Some((path.to_owned(), load_details(context, path.to_owned())));
        }

        let dialog = egui::Modal::new(egui::Id::unique("image_details")).show(context, |ui| {
            ui.set_width(470.0);
            ui.horizontal(|ui| {
                ui.heading(catalog.text(184));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::close(ui, catalog.text(142)).clicked() {
                        ui.close_kind(egui::UiKind::Modal);
                    }
                });
            });
            let height = (context.content_rect().height() - 115.0).max(100.0);
            egui::ScrollArea::vertical()
                .id_salt(path)
                .max_height(height)
                .show(ui, |ui| match &self.details {
                    Some(Ok(details)) => {
                        show_details(ui, catalog, path, details, self.preview.as_ref());
                    }
                    Some(Err(error)) => show_error(ui, catalog, error),
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(catalog.text(195));
                        });
                    }
                });
        });
        if dialog.should_close() {
            self.path = None;
            self.details = None;
            self.preview = None;
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

fn show_details(
    ui: &mut Ui,
    catalog: &Catalog,
    path: &Path,
    details: &FileDetails,
    preview: Option<&TextureHandle>,
) {
    ui.vertical_centered(|ui| {
        if let Some(preview) = preview {
            ui.add(egui::Image::new(preview).max_height(225.0).max_width(410.0));
        } else {
            ui.weak(catalog.text(193));
        }
    });
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
            ui.label(file_name(path)).on_hover_text(path_label(path));
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

    ui.separator();
    ui.heading(catalog.text(191));
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
