use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

use eframe::egui::{ColorImage, Context};
use tenka_core::{BENCHMARK_SETTINGS, MAX_INPUT_BYTES, Options, OutputFormat, Setting, Source};

pub enum Event {
    Started(usize),
    Converted(usize, Result<Conversion, Failure>),
    Original(ColorImage, String),
    Measured(Setting, Result<Comparison, Failure>),
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

pub struct Comparison {
    pub bytes: usize,
    pub quality: Option<f64>,
    pub milliseconds: f64,
    pub alpha_exact: bool,
    pub preview: ColorImage,
}

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

pub fn compare(context: &Context, file: PathBuf) -> Receiver<Event> {
    run(context, move |emit| {
        let data = match read_image(&file)
            .and_then(|bytes| Source::decode(&bytes).map_err(Failure::from))
        {
            Ok(result) => result,
            Err(error) => {
                emit(Event::Failed(error));
                return;
            }
        };
        let (width, height) = data.dimensions();
        let thumbnail = data.preview();
        let original = ColorImage::from_rgba_unmultiplied(
            [thumbnail.width() as usize, thumbnail.height() as usize],
            thumbnail.as_raw(),
        );
        emit(Event::Original(original, format!("{width} × {height}")));
        for setting in BENCHMARK_SETTINGS {
            let result = data
                .measure(setting)
                .map(|measurement| {
                    let thumbnail = measurement.preview;
                    Comparison {
                        bytes: measurement.bytes,
                        quality: measurement.quality,
                        milliseconds: measurement.encode_time.as_secs_f64() * 1000.0,
                        alpha_exact: measurement.alpha_exact,
                        preview: ColorImage::from_rgba_unmultiplied(
                            [thumbnail.width() as usize, thumbnail.height() as usize],
                            thumbnail.as_raw(),
                        ),
                    }
                })
                .map_err(Failure::from);
            emit(Event::Measured(setting, result));
        }
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
    let path =
        save_image(path, folder, output.setting.format(), &output.bytes).map_err(|error| {
            Failure {
                label: 60,
                detail: error.to_string(),
            }
        })?;
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
    bytes: &[u8],
) -> io::Result<PathBuf> {
    let folder = folder
        .or_else(|| source.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = source
        .file_stem()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing file name"))?;
    let mut temp = tempfile::NamedTempFile::new_in(folder)?;
    temp.write_all(bytes)?;
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
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Too many output files with this name",
    ))
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
