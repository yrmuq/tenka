use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use thiserror::Error;

use super::{FileConversion, FileError, SourcePolicy, convert_file_to};
use crate::{Error as CodecError, OptimizationMode, Options};

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Default)]
pub struct FolderScan {
    pub files: Vec<PathBuf>,
    pub errors: Vec<ScanError>,
}

#[derive(Debug, Error)]
#[error("{path}: {error}", path = .path.display())]
pub struct ScanError {
    pub path: PathBuf,
    #[source]
    pub error: io::Error,
}

/// Scan supported images in a folder and its descendants, without following symlinks inside it.
pub fn scan_folder(folder: &Path) -> FolderScan {
    scan(folder, None, &AtomicBool::new(false))
}

fn scan(root: &Path, exclude: Option<&Path>, cancelled: &AtomicBool) -> FolderScan {
    let mut found = FolderScan::default();
    let mut folders = vec![root.to_owned()];
    'scan: while let Some(folder) = folders.pop() {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) => {
                found.errors.push(ScanError {
                    path: folder,
                    error,
                });
                continue;
            }
        };
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {
                break 'scan;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    found.errors.push(ScanError {
                        path: folder.clone(),
                        error,
                    });
                    continue;
                }
            };
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && exclude != Some(path.as_path()) => folders.push(path),
                Ok(kind) if kind.is_file() && is_supported_image(&path) => found.files.push(path),
                Ok(_) => {}
                Err(error) => found.errors.push(ScanError { path, error }),
            }
        }
    }
    found.files.sort_unstable();
    found
        .errors
        .sort_by(|left, right| left.path.cmp(&right.path));
    found
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

#[derive(Debug, Error)]
pub enum SubfolderError {
    #[error("Invalid relative output path {0}")]
    InvalidRelativePath(PathBuf),
    #[error("Could not create destination subfolder {path}: {error}", path = .path.display())]
    Create { path: PathBuf, error: io::Error },
}

/// Create a relative directory tree under an existing destination root.
///
/// Existing symlinks within the relative path are refused.
pub fn create_output_subfolder(root: &Path, relative: &Path) -> Result<PathBuf, SubfolderError> {
    let mut folder = root.to_owned();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(SubfolderError::InvalidRelativePath(relative.to_owned()));
        };
        folder.push(name);
        match fs::create_dir(&folder) {
            Ok(()) => {}
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    && fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir()) => {}
            Err(error) => {
                return Err(SubfolderError::Create {
                    path: folder,
                    error,
                });
            }
        }
    }
    Ok(folder)
}

#[derive(Debug, Error)]
pub enum FolderError {
    #[error("Could not open source folder")]
    Source(#[source] io::Error),
    #[error("Could not resolve destination folder")]
    Destination(#[source] io::Error),
}

#[derive(Debug)]
pub struct ImageResult {
    pub source: PathBuf,
    pub result: Result<FileConversion, FileError>,
}

#[derive(Debug)]
pub struct FolderReport {
    /// Number of supported files discovered during the scan.
    pub total: usize,
    pub scan_errors: Vec<ScanError>,
    pub images: Vec<ImageResult>,
    pub elapsed: Duration,
    pub cancelled: bool,
}

#[derive(Debug)]
pub enum FolderProgress<'a> {
    Scanning,
    Scanned {
        total: usize,
    },
    Converting {
        source: &'a Path,
        completed: usize,
        total: usize,
    },
    FinishedFile {
        image: &'a ImageResult,
        completed: usize,
        total: usize,
    },
}

/// Convert the supported images in a folder and its subfolders.
///
/// Copy the relative subfolder layout directly into `destination`, creating folders
/// as needed. Skip an existing destination tree nested in `source`. A file failure
/// is recorded in the report and processing continues. On cancellation the report
/// contains results for files already finished.
pub fn convert_folder(
    source: &Path,
    destination: &Path,
    options: Options,
    mode: OptimizationMode,
    source_policy: SourcePolicy,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(FolderProgress<'_>),
) -> Result<FolderReport, FolderError> {
    let start = Instant::now();
    let source = fs::canonicalize(source).map_err(FolderError::Source)?;
    if !fs::metadata(&source).map_err(FolderError::Source)?.is_dir() {
        return Err(FolderError::Source(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Expected a source directory",
        )));
    }
    let excluded = match fs::canonicalize(destination) {
        Ok(path) => {
            if !fs::metadata(&path)
                .map_err(FolderError::Destination)?
                .is_dir()
            {
                return Err(FolderError::Destination(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "Expected a destination directory",
                )));
            }
            (path != source && path.starts_with(&source)).then_some(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(FolderError::Destination(error)),
    };
    progress(FolderProgress::Scanning);
    let FolderScan { files, errors } = scan(&source, excluded.as_deref(), cancelled);
    let total = files.len();
    progress(FolderProgress::Scanned { total });
    let mut images = Vec::with_capacity(total);
    let mut interrupted = false;
    for path in files {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        progress(FolderProgress::Converting {
            source: &path,
            completed: images.len(),
            total,
        });
        let result = convert_file_to(&path, options, mode, source_policy, cancelled, || {
            let parent = path.parent().unwrap_or(&source);
            let relative = parent.strip_prefix(&source).map_err(|error| {
                FileError::Destination(io::Error::new(io::ErrorKind::InvalidInput, error))
            })?;
            fs::create_dir_all(destination).map_err(FileError::Destination)?;
            create_output_subfolder(destination, relative)
                .map_err(|error| FileError::Destination(io::Error::other(error)))
        });
        if matches!(result, Err(FileError::Conversion(CodecError::Cancelled))) {
            interrupted = true;
            break;
        }
        let image = ImageResult {
            source: path,
            result,
        };
        progress(FolderProgress::FinishedFile {
            image: &image,
            completed: images.len() + 1,
            total,
        });
        images.push(image);
    }
    Ok(FolderReport {
        total,
        scan_errors: errors,
        images,
        elapsed: start.elapsed(),
        cancelled: interrupted || cancelled.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_keeps_relative_paths_and_reports_errors_without_stopping() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("photos");
        let out = root.join("output");
        fs::create_dir_all(root.join("album")).unwrap();
        fs::create_dir_all(out.join("album")).unwrap();
        fs::write(root.join("a-bad.png"), b"broken").unwrap();
        fs::write(out.join("old.webp"), b"not part of the source batch").unwrap();
        fs::write(out.join("album/frame.jxl"), b"existing").unwrap();
        fs::write(root.join("notes.txt"), b"not an image").unwrap();
        let source = root.join("album/frame.PNG");
        image::RgbImage::from_pixel(4, 4, image::Rgb([12, 34, 56]))
            .save(&source)
            .unwrap();

        let mut finished = Vec::new();
        let report = convert_folder(
            &root,
            &out,
            Options::default(),
            OptimizationMode::Best,
            SourcePolicy::Keep,
            &AtomicBool::new(false),
            |event| {
                if let FolderProgress::FinishedFile {
                    completed, total, ..
                } = event
                {
                    finished.push((completed, total));
                }
            },
        )
        .unwrap();

        assert_eq!(report.total, 2);
        assert!(report.scan_errors.is_empty());
        assert!(report.images[0].result.is_err());
        let saved = report.images[1]
            .result
            .as_ref()
            .unwrap()
            .output
            .as_ref()
            .unwrap();
        assert_eq!(saved.path, out.join("album/frame-1.jxl"));
        assert!(saved.path.is_file());
        assert!(source.is_file());
        assert_eq!(fs::read(out.join("album/frame.jxl")).unwrap(), b"existing");
        assert_eq!(finished, [(1, 2), (2, 2)]);
    }

    #[test]
    fn cancellation_reports_only_the_finished_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let out = root.join("output");
        for name in ["a.png", "b.png"] {
            image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
                .save(root.join(name))
                .unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let report = convert_folder(
            root,
            &out,
            Options::default(),
            OptimizationMode::Best,
            SourcePolicy::DeleteAfterSave,
            &cancelled,
            |event| {
                if matches!(event, FolderProgress::FinishedFile { completed: 1, .. }) {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        assert!(report.cancelled);
        assert_eq!(report.total, 2);
        assert_eq!(report.images.len(), 1);
        assert!(out.join("a.jxl").is_file());
        assert!(!root.join("a.png").exists());
        assert!(root.join("b.png").is_file());
        assert!(!out.join("b.jxl").exists());
    }
}
