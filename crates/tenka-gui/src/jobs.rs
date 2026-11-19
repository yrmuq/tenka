use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

use eframe::egui::{Color32, ColorImage, Context, Rect, pos2};
use tenka_core::{MAX_INPUT_BYTES, OptimizationMode, Options, OutputFormat, Source};

pub enum Event {
    Started(usize),
    Converted(usize, Result<Option<Conversion>, Failure>),
    Finished,
}

pub struct Conversion {
    pub path: PathBuf,
    pub source_bytes: usize,
    pub bytes: usize,
    pub dimensions: (u32, u32),
    pub quality: f64,
    pub lossless: bool,
}

pub struct ComparisonPixels {
    pub dimensions: [usize; 2],
    pub tiles: Vec<(Rect, ColorImage)>,
}

#[derive(Debug)]
pub struct Failure {
    pub label: u16,
    pub detail: String,
}

impl From<tenka_core::Error> for Failure {
    fn from(error: tenka_core::Error) -> Self {
        use tenka_core::Error::*;
        let label = match error {
            Format => 48,
            Animation => 49,
            BitDepth => 50,
            Limits => 51,
            Color => 52,
            Options => 53,
            Decode(_) => 54,
            JxlEncode(_) => 55,
            JxlDecode(_) => 56,
            WebpEncode => 57,
            Measurement => 58,
            WebpConfig => 77,
            WebpCandidateDecode(_) => 78,
            JxlPixels => 79,
            QualitySample => 80,
            CandidateDimensions => 81,
            QualityRange(_) => 86,
        };
        Self {
            label,
            detail: std::error::Error::source(&error).map_or_else(String::new, ToString::to_string),
        }
    }
}

pub fn convert(
    context: &Context,
    files: Vec<(usize, PathBuf)>,
    output_folder: Option<PathBuf>,
    options: Options,
    mode: OptimizationMode,
) -> Receiver<Event> {
    run(context, move |emit| {
        for (idx, path) in files {
            emit(Event::Started(idx));
            emit(Event::Converted(
                idx,
                convert_one(&path, output_folder.as_deref(), options, mode),
            ));
        }
    })
}

pub fn load_comparison(
    context: &Context,
    file: PathBuf,
    texture_limit: usize,
) -> Receiver<Result<ComparisonPixels, Failure>> {
    let context = context.clone();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = read_image(&file)
            .and_then(|bytes| tenka_core::decode_for_comparison(&bytes).map_err(Failure::from))
            .map(|image| {
                let width = image.width() as usize;
                let height = image.height() as usize;
                let pixels = image.as_raw();
                let mut tiles = Vec::new();
                for top in (0..height).step_by(texture_limit) {
                    let bottom = (top + texture_limit).min(height);
                    for left in (0..width).step_by(texture_limit) {
                        let right = (left + texture_limit).min(width);
                        let bounds = Rect::from_min_max(
                            pos2(left as f32 / width as f32, top as f32 / height as f32),
                            pos2(right as f32 / width as f32, bottom as f32 / height as f32),
                        );
                        let size = [right - left, bottom - top];
                        let tile = if left == 0 && right == width {
                            ColorImage::from_rgba_unmultiplied(
                                size,
                                &pixels[top * width * 4..bottom * width * 4],
                            )
                        } else {
                            let colors = pixels
                                .chunks_exact(width * 4)
                                .skip(top)
                                .take(size[1])
                                .flat_map(|row| row[left * 4..right * 4].as_chunks::<4>().0)
                                .map(|&[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a))
                                .collect();
                            ColorImage::new(size, colors)
                        };
                        tiles.push((bounds, tile));
                    }
                }
                ComparisonPixels {
                    dimensions: [width, height],
                    tiles,
                }
            });
        if send.send(result).is_ok() {
            context.request_repaint();
        }
    });
    receive
}

fn run(context: &Context, work: impl FnOnce(&dyn Fn(Event)) + Send + 'static) -> Receiver<Event> {
    let context = context.clone();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let emit = |event| {
            if send.send(event).is_ok() {
                context.request_repaint();
            }
        };
        work(&emit);
        emit(Event::Finished);
    });
    receive
}

fn convert_one(
    path: &Path,
    folder: Option<&Path>,
    options: Options,
    mode: OptimizationMode,
) -> Result<Option<Conversion>, Failure> {
    let bytes = read_image(path)?;
    let source = Source::decode(&bytes)?;
    let Some(output) = source.optimize_with_mode(options, bytes.len(), mode)? else {
        return Ok(None);
    };
    let path = save_image(path, folder, output.setting.format(), &output.bytes)?;
    Ok(Some(Conversion {
        path,
        source_bytes: bytes.len(),
        bytes: output.bytes.len(),
        dimensions: output.dimensions,
        quality: output.quality,
        lossless: output.setting.is_lossless() && output.dimensions == source.dimensions(),
    }))
}

fn read_image(path: &Path) -> Result<Vec<u8>, Failure> {
    let result = (|| {
        if !fs::metadata(path)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a regular file",
            ));
        }
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    })();
    result.map_err(|error: io::Error| Failure {
        label: 59,
        detail: error.to_string(),
    })
}

fn save_image(
    source: &Path,
    folder: Option<&Path>,
    format: OutputFormat,
    bytes: &[u8],
) -> Result<PathBuf, Failure> {
    let folder = folder
        .or_else(|| source.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = source.file_stem().ok_or(Failure {
        label: 95,
        detail: source.display().to_string(),
    })?;
    let failed = |label, error: io::Error| Failure {
        label,
        detail: error.to_string(),
    };
    let mut temp = tempfile::NamedTempFile::new_in(folder).map_err(|error| failed(96, error))?;
    temp.write_all(bytes).map_err(|error| failed(97, error))?;
    for i in 0..1000 {
        let mut name = OsString::from(stem);
        if i > 0 {
            name.push(format!("-{i}"));
        }
        name.push(".");
        name.push(format.extension());
        let path = folder.join(name);
        match temp.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => temp = error.file,
            Err(error) => return Err(failed(60, error.error)),
        }
    }
    Err(Failure {
        label: 98,
        detail: folder.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_the_same_format_preserves_the_input_and_existing_outputs() {
        let folder = tempfile::tempdir().unwrap();
        let input = folder.path().join("image.webp");
        fs::write(&input, b"input").unwrap();
        let first = save_image(&input, None, OutputFormat::Webp, b"one").unwrap();
        let second = save_image(&input, None, OutputFormat::Webp, b"two").unwrap();
        assert_eq!(fs::read(input).unwrap(), b"input");
        assert_eq!(first.file_name().unwrap(), "image-1.webp");
        assert_eq!(fs::read(first).unwrap(), b"one");
        assert_eq!(second.file_name().unwrap(), "image-2.webp");
        assert_eq!(fs::read(&second).unwrap(), b"two");
        let jxl = save_image(&second, None, OutputFormat::JpegXl, b"three").unwrap();
        assert_eq!(jxl.file_name().unwrap(), "image-2.jxl");
    }
}
