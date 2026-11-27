use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    ops::ControlFlow::{self, Break, Continue},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

use eframe::egui::{Color32, ColorImage, Context, Rect, pos2};
use tenka_core::{MAX_INPUT_BYTES, OptimizationMode, Options, OutputFormat, Source};

use crate::estimate::{InputSize, Timing};

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Clone)]
pub struct InputFile {
    pub path: PathBuf,
    relative_folder: PathBuf,
}

impl From<PathBuf> for InputFile {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            relative_folder: PathBuf::new(),
        }
    }
}

pub struct ScannedInputs {
    pub files: Vec<InputFile>,
    pub failure: Option<Failure>,
}

pub enum Event {
    Started(usize),
    Converted(usize, Result<CompletedConversion, Failure>),
    Interrupted(usize),
    Finished(Duration),
}

pub struct CompletedConversion {
    pub output: Option<Conversion>,
    pub timing: Timing,
}

pub struct Conversion {
    pub path: PathBuf,
    pub source_bytes: usize,
    pub bytes: usize,
}

pub struct InputProbe {
    pub path: PathBuf,
    pub result: Weak<OnceLock<InputSize>>,
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
            Cancelled => 131,
            Format => 48,
            Animation => 49,
            BitDepth => 50,
            Limits => 51,
            Color => 52,
            Options => 53,
            Decode(_) => 54,
            JxlEncode(_) => 55,
            JxlTranscode(_) => 132,
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
    files: Vec<(usize, InputFile)>,
    output_folder: Option<PathBuf>,
    options: Options,
    mode: OptimizationMode,
    cancelled: Arc<AtomicBool>,
) -> Receiver<Event> {
    run(context, move |emit| {
        for (idx, input) in files {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            emit(Event::Started(idx));
            let result =
                match convert_one(&input, output_folder.as_deref(), options, mode, &cancelled) {
                    Ok(Continue(result)) => Ok(result),
                    Ok(Break(())) => {
                        emit(Event::Interrupted(idx));
                        break;
                    }
                    Err(error) => Err(error),
                };
            emit(Event::Converted(idx, result));
        }
    })
}

pub fn input_scanner(context: &Context) -> (Sender<Vec<PathBuf>>, Receiver<ScannedInputs>) {
    let context = context.clone();
    let (requests, scan) = mpsc::channel::<Vec<PathBuf>>();
    let (send, results) = mpsc::channel();
    std::thread::spawn(move || {
        for paths in scan {
            if send.send(scan_inputs(paths)).is_err() {
                break;
            }
            context.request_repaint();
        }
    });
    (requests, results)
}

fn scan_inputs(paths: Vec<PathBuf>) -> ScannedInputs {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        if !path.is_dir() {
            files.push(InputFile::from(path));
            continue;
        }
        let start = files.len();
        let root_name = path.file_name().map(PathBuf::from).unwrap_or_default();
        let mut folders = vec![(path, root_name)];
        while let Some((folder, relative_folder)) = folders.pop() {
            let entries = match fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(error) => {
                    errors.push(format!("{}: {error}", folder.display()));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        errors.push(format!("{}: {error}", folder.display()));
                        continue;
                    }
                };
                let path = entry.path();
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => {
                        folders.push((path, relative_folder.join(entry.file_name())));
                    }
                    Ok(kind) if kind.is_file() && is_supported_image(&path) => {
                        files.push(InputFile {
                            path,
                            relative_folder: relative_folder.clone(),
                        });
                    }
                    Ok(_) => {}
                    Err(error) => errors.push(format!("{}: {error}", path.display())),
                }
            }
        }
        files[start..].sort_unstable_by(|a, b| a.path.cmp(&b.path));
    }
    let failure = (!errors.is_empty()).then(|| Failure {
        label: 145,
        detail: errors.join("\n"),
    });
    ScannedInputs { files, failure }
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|supported| ext.eq_ignore_ascii_case(supported))
        })
}

pub fn probe_inputs() -> Sender<Vec<InputProbe>> {
    let (send, receive) = mpsc::channel::<Vec<InputProbe>>();
    std::thread::spawn(move || {
        for probes in receive {
            for probe in probes {
                if let Some(result) = probe.result.upgrade() {
                    let _ = result.set(probe_input(&probe.path));
                }
            }
        }
    });
    send
}

pub fn reveal_file(context: &Context, path: PathBuf) -> Receiver<Result<(), Failure>> {
    file_manager_request(context, path, "ShowItems", 133)
}

pub fn open_folder(context: &Context, path: PathBuf) -> Receiver<Result<(), Failure>> {
    file_manager_request(context, path, "ShowFolders", 71)
}

fn file_manager_request(
    context: &Context,
    path: PathBuf,
    method: &'static str,
    label: u16,
) -> Receiver<Result<(), Failure>> {
    let context = context.clone();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = show_in_file_manager(&path, method).map_err(|detail| Failure {
            label,
            detail: format!("{}\n{detail}", path.display()),
        });
        if send.send(result).is_ok() {
            context.request_repaint();
        }
    });
    receive
}

#[cfg(target_os = "linux")]
fn show_in_file_manager(path: &Path, method: &str) -> Result<(), String> {
    use zbus::{Connection, Proxy, block_on};

    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let uri = url::Url::from_file_path(absolute)
        .map_err(|()| "The file path could not be converted to a URI".to_owned())?;
    let uris = [uri.as_str()];
    let result: zbus::Result<()> = block_on(async {
        let connection = Connection::session().await?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
        )
        .await?;
        proxy.call(method, &(uris.as_slice(), "")).await
    });
    result.map_err(|error| error.to_string())
}

#[cfg(not(target_os = "linux"))]
fn show_in_file_manager(_: &Path, _: &str) -> Result<(), String> {
    Err("Showing files in the desktop file manager is supported on Linux".into())
}

fn probe_input(path: &Path) -> InputSize {
    let Some(metadata) = fs::metadata(path).ok().filter(fs::Metadata::is_file) else {
        return InputSize::default();
    };
    let bytes = metadata.len();
    let pixels = if bytes <= MAX_INPUT_BYTES {
        File::open(path)
            .ok()
            .and_then(|file| Source::probe_dimensions(BufReader::new(file)))
            .map(|(width, height)| u64::from(width) * u64::from(height))
    } else {
        None
    };
    InputSize {
        bytes: Some(bytes.min(MAX_INPUT_BYTES + 1)),
        pixels,
    }
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
        let start = Instant::now();
        let emit = |event| {
            if send.send(event).is_ok() {
                context.request_repaint();
            }
        };
        work(&emit);
        emit(Event::Finished(start.elapsed()));
    });
    receive
}

fn convert_one(
    source: &InputFile,
    folder: Option<&Path>,
    options: Options,
    mode: OptimizationMode,
    cancelled: &AtomicBool,
) -> Result<ControlFlow<(), CompletedConversion>, Failure> {
    let start = Instant::now();
    let input = read_image(&source.path);
    let read = start.elapsed();
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Break(()));
    }
    let bytes = input?;
    let output = tenka_core::optimize_for_conversion(&bytes, options, mode, cancelled);
    if cancelled.load(Ordering::Relaxed) || matches!(output, Err(tenka_core::Error::Cancelled)) {
        return Ok(Break(()));
    }
    let ((width, height), output) = output?;
    let output = match output {
        Some(output) => {
            let mirrored = folder
                .filter(|_| !source.relative_folder.as_os_str().is_empty())
                .map(|root| create_output_subfolder(root, &source.relative_folder))
                .transpose()?;
            Some(Conversion {
                path: save_image(
                    &source.path,
                    mirrored.as_deref().or(folder),
                    output.setting.format(),
                    &output.bytes,
                )?,
                source_bytes: bytes.len(),
                bytes: output.bytes.len(),
            })
        }
        None => None,
    };
    Ok(Continue(CompletedConversion {
        output,
        timing: Timing {
            bytes: bytes.len() as u64,
            pixels: u64::from(width) * u64::from(height),
            read,
            processing: start.elapsed() - read,
        },
    }))
}

fn create_output_subfolder(root: &Path, relative: &Path) -> Result<PathBuf, Failure> {
    let mut folder = root.to_owned();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(Failure {
                label: 146,
                detail: relative.display().to_string(),
            });
        };
        folder.push(name);
        match fs::create_dir(&folder) {
            Ok(()) => {}
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    && fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir()) => {}
            Err(error) => {
                return Err(Failure {
                    label: 147,
                    detail: format!("{}\n{error}", folder.display()),
                });
            }
        }
    }
    Ok(folder)
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
    fn imported_folders_keep_their_structure_and_existing_outputs() {
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00,
            0x00, 0xfd, 0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x10, 0x31, 0x72, 0x4b, 0xe9, 0x39, 0xc1, 0x00, 0xa1, 0x00, 0x1b, 0x16,
            0x04, 0x89, 0x7e, 0x7c, 0xdd, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
            0xae, 0x42, 0x60, 0x82,
        ];
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Фото");
        let destination = temp.path().join("Вывод");
        fs::create_dir_all(source.join("Отпуск/День 1")).unwrap();
        fs::write(source.join("кадр.PNG"), PNG).unwrap();
        fs::write(source.join("Отпуск/День 1/кадр.PNG"), PNG).unwrap();
        fs::write(source.join("Отпуск/notes.txt"), b"notes").unwrap();
        let existing = destination.join("Фото/кадр.jxl");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, b"existing").unwrap();

        let scanned = scan_inputs(vec![source.clone()]);
        assert!(scanned.failure.is_none());
        assert_eq!(scanned.files.len(), 2);
        let convert = |input, folder| {
            let Continue(completed) = convert_one(
                input,
                folder,
                Options::default(),
                OptimizationMode::Best,
                &AtomicBool::new(false),
            )
            .unwrap() else {
                panic!("conversion was interrupted");
            };
            completed.output.unwrap().path
        };
        let mut outputs: Vec<_> = scanned
            .files
            .iter()
            .map(|input| convert(input, Some(&destination)))
            .collect();
        outputs.sort();
        assert_eq!(
            outputs,
            [
                destination.join("Фото/Отпуск/День 1/кадр.jxl"),
                destination.join("Фото/кадр-1.jxl"),
            ]
        );
        assert_eq!(fs::read(&existing).unwrap(), b"existing");
        let input = scanned
            .files
            .iter()
            .find(|input| input.path == source.join("кадр.PNG"))
            .unwrap();
        assert_eq!(convert(input, None), source.join("кадр.jxl"));
        assert_eq!(fs::read(&input.path).unwrap(), PNG);
    }

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
