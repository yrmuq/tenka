use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Instant,
};

use eframe::egui::{Color32, ColorImage, Context, Rect, pos2};
use tenka_core::{MAX_INPUT_BYTES, Options, OutputFormat, QualityTest, Setting, Source};

pub enum Event {
    Started(usize),
    Converted(usize, Result<Conversion, Failure>),
    Original(ColorImage, String),
    Tested(u8, Result<TestResult, Failure>),
    Failed(Failure),
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

pub struct TestResult {
    pub path: PathBuf,
    pub setting: Setting,
    pub bytes: usize,
    pub dimensions: (u32, u32),
    pub quality: f64,
    pub milliseconds: f64,
    pub preview: ColorImage,
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
            QualityCeiling(_) => 105,
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
) -> Receiver<Event> {
    run(context, move |emit| {
        for (idx, path) in files {
            emit(Event::Started(idx));
            emit(Event::Converted(
                idx,
                convert_one(&path, output_folder.as_deref(), options),
            ));
        }
    })
}

pub fn test_qualities(
    context: &Context,
    file: PathBuf,
    folder: Option<PathBuf>,
    options: Options,
    qualities: Vec<u8>,
) -> Receiver<Event> {
    run(context, move |emit| {
        let source = match read_image(&file)
            .and_then(|bytes| Source::decode(&bytes).map_err(Failure::from))
        {
            Ok(result) => result,
            Err(error) => {
                emit(Event::Failed(error));
                return;
            }
        };
        let (width, height) = source.dimensions();
        let thumbnail = source.preview();
        let original = ColorImage::from_rgba_unmultiplied(
            [thumbnail.width() as usize, thumbnail.height() as usize],
            thumbnail.as_raw(),
        );
        emit(Event::Original(original, format!("{width} × {height}")));
        let mut test = source.quality_test(options);
        for quality in qualities {
            emit(Event::Tested(
                quality,
                test_one(&mut test, &file, folder.as_deref(), quality),
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

fn test_one(
    test: &mut QualityTest<'_>,
    file: &Path,
    folder: Option<&Path>,
    ceiling: u8,
) -> Result<TestResult, Failure> {
    let start = Instant::now();
    let result = test.encode(ceiling)?;
    let milliseconds = start.elapsed().as_secs_f64() * 1000.0;
    let thumbnail = result.preview()?;
    let path = save_image(
        file,
        folder,
        result.setting.format(),
        Some(ceiling),
        &result.bytes,
    )?;
    Ok(TestResult {
        path,
        setting: result.setting,
        bytes: result.bytes.len(),
        dimensions: result.dimensions,
        quality: result.quality,
        milliseconds,
        preview: ColorImage::from_rgba_unmultiplied(
            [thumbnail.width() as usize, thumbnail.height() as usize],
            thumbnail.as_raw(),
        ),
    })
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
) -> Result<Conversion, Failure> {
    let bytes = read_image(path)?;
    let source = Source::decode(&bytes)?;
    let output = source.convert(options)?;
    let path = save_image(path, folder, output.setting.format(), None, &output.bytes)?;
    Ok(Conversion {
        path,
        source_bytes: bytes.len(),
        bytes: output.bytes.len(),
        dimensions: output.dimensions,
        quality: output.quality,
        lossless: output.setting.is_lossless() && output.dimensions == source.dimensions(),
    })
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
    quality_suffix: Option<u8>,
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
    let suffix = quality_suffix.map(|value| format!("-{value}"));
    for i in 0..1000 {
        let mut name = OsString::from(stem);
        if i > 0 {
            name.push(format!("-{i}"));
        }
        if let Some(suffix) = &suffix {
            name.push(suffix);
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
        let first = save_image(&input, None, OutputFormat::Webp, None, b"one").unwrap();
        let second = save_image(&input, None, OutputFormat::Webp, None, b"two").unwrap();
        assert_eq!(fs::read(input).unwrap(), b"input");
        assert_eq!(first.file_name().unwrap(), "image-1.webp");
        assert_eq!(fs::read(first).unwrap(), b"one");
        assert_eq!(second.file_name().unwrap(), "image-2.webp");
        assert_eq!(fs::read(&second).unwrap(), b"two");
        let jxl = save_image(&second, None, OutputFormat::JpegXl, None, b"three").unwrap();
        assert_eq!(jxl.file_name().unwrap(), "image-2.jxl");
    }

    #[test]
    fn quality_batch_writes_only_the_selected_format_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let input = root.path().join("image.png");
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 176, 113,
            11, 0, 0, 1, 148, 0, 211, 246, 194, 60, 171, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ];
        fs::write(&input, png).unwrap();
        let existing = destination.path().join("image-100.webp");
        fs::write(&existing, b"existing output").unwrap();

        for (format, folder, expected_path) in [
            (OutputFormat::JpegXl, None, "image-100.jxl"),
            (
                OutputFormat::Webp,
                Some(destination.path().to_owned()),
                "image-1-100.webp",
            ),
        ] {
            let expected_folder = folder.as_deref().unwrap_or(root.path()).to_owned();
            let events = test_qualities(
                &Context::default(),
                input.clone(),
                folder,
                Options {
                    format,
                    ..Options::default()
                },
                vec![87, 100],
            );
            let mut saved = Vec::new();
            let mut rejected = false;
            loop {
                match events
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap()
                {
                    Event::Tested(87, Err(error)) => {
                        assert_eq!(error.label, 105);
                        rejected = true;
                    }
                    Event::Tested(quality, result) => {
                        let result = result.unwrap();
                        assert_eq!(result.setting.format(), format);
                        assert!(result.setting.is_lossless());
                        assert_eq!(result.path.parent(), Some(expected_folder.as_path()));
                        assert_eq!(
                            fs::metadata(&result.path).unwrap().len(),
                            result.bytes as u64
                        );
                        saved.push((quality, result.path));
                    }
                    Event::Original(..) => {}
                    Event::Finished => break,
                    _ => panic!("unexpected event during quality batch"),
                }
            }
            assert!(rejected);
            assert_eq!(saved, vec![(100, expected_folder.join(expected_path))]);
            assert!(
                !expected_folder
                    .join(format!("image-87.{}", format.extension()))
                    .exists()
            );
        }
        assert_eq!(fs::read(existing).unwrap(), b"existing output");
        assert_eq!(fs::read(input).unwrap(), png);
        assert!(!root.path().join("image-100.webp").exists());
        assert!(!destination.path().join("image-100.jxl").exists());
    }
}
