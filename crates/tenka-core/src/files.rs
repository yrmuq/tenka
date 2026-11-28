//! Convert files into a caller-selected directory using the desktop conversion policy.

use std::{
    ffi::OsString,
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{
    MAX_INPUT_BYTES, OptimizationMode, Options, OutputFormat, Setting, check_cancelled,
    optimize_for_conversion,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourcePolicy {
    #[default]
    Keep,
    DeleteAfterSave,
}

#[derive(Debug)]
pub struct FileConversion {
    pub source_bytes: u64,
    pub source_dimensions: (u32, u32),
    /// Absent when no output satisfied the conversion policy.
    pub output: Option<SavedImage>,
}

#[derive(Debug)]
pub struct SavedImage {
    pub path: PathBuf,
    pub bytes: u64,
    pub dimensions: (u32, u32),
    pub quality: f64,
    pub setting: Setting,
    pub source_status: SourceStatus,
}

#[derive(Debug)]
pub enum SourceStatus {
    Kept,
    Deleted,
    /// Cancellation was requested after the output was written.
    KeptAfterCancellation,
    /// The output was written and the source could not safely be removed.
    DeletionFailed(RemoveError),
}

#[derive(Debug, Error)]
pub enum FileError {
    #[error("Could not read source image")]
    Read(#[source] io::Error),
    #[error(transparent)]
    Conversion(#[from] crate::Error),
    #[error("Could not create output directory")]
    Destination(#[source] io::Error),
    #[error(transparent)]
    Save(#[from] SaveError),
}

#[derive(Debug, Error)]
pub enum SaveError {
    #[error("Source path has no file stem")]
    MissingFileStem,
    #[error("Could not create temporary output")]
    Create(#[source] io::Error),
    #[error("Could not write output")]
    Write(#[source] io::Error),
    #[error("Could not publish output")]
    Publish(#[source] io::Error),
    #[error("No unused output filename found in {0}")]
    NameExhausted(PathBuf),
}

#[derive(Debug, Error)]
pub enum RemoveError {
    #[error("Could not sync the output directory")]
    SyncDirectory(#[source] io::Error),
    #[error("Could not check the source before deleting it")]
    CheckSource(#[source] io::Error),
    #[error("The source changed or is a symbolic link")]
    ChangedSource,
    #[error("Could not delete the source")]
    Delete(#[source] io::Error),
}

/// Convert a single image, creating the output directory if needed.
///
/// Existing files are preserved by choosing a numbered name on collision.
/// With [SourcePolicy::DeleteAfterSave], removal is attempted only after
/// publication. A skipped conversion keeps its source. If the output was
/// saved and removal failed, the result contains both the output and the failure.
/// Callers must coordinate with any other process that renames or writes the source.
///
/// ```no_run
/// use std::{path::Path, sync::atomic::AtomicBool};
/// use tenka_core::{OptimizationMode, Options, OutputFormat};
/// use tenka_core::files::{SourcePolicy, convert_file};
///
/// let result = convert_file(
///     Path::new("photo.jpg"),
///     Path::new("converted"),
///     Options { format: OutputFormat::JpegXl, auto_resize: false },
///     OptimizationMode::Cheap,
///     SourcePolicy::Keep,
///     &AtomicBool::new(false),
/// )?;
/// if let Some(output) = result.output {
///     println!("{}", output.path.display());
/// }
/// # Ok::<(), tenka_core::files::FileError>(())
/// ```
pub fn convert_file(
    source: &Path,
    destination: &Path,
    options: Options,
    mode: OptimizationMode,
    source_policy: SourcePolicy,
    cancelled: &AtomicBool,
) -> Result<FileConversion, FileError> {
    check_cancelled(Some(cancelled))?;
    let file = open_image(source).map_err(FileError::Read)?;
    let delete = source_policy == SourcePolicy::DeleteAfterSave;
    let original = delete
        .then(|| file.metadata())
        .transpose()
        .map_err(FileError::Read)?;
    let input = read_bounded(&file).map_err(FileError::Read)?;
    let (source_dimensions, output) = optimize_for_conversion(&input, options, mode, cancelled)?;
    check_cancelled(Some(cancelled))?;
    let output = if let Some(output) = output {
        fs::create_dir_all(destination).map_err(FileError::Destination)?;
        let path = save_bytes(
            source,
            destination,
            output.setting.format(),
            &output.bytes,
            delete,
        )?;
        let source_status = match original {
            None => SourceStatus::Kept,
            Some(_) if cancelled.load(Ordering::Relaxed) => SourceStatus::KeptAfterCancellation,
            Some(original) => match remove_source(source, &original, destination, cancelled) {
                Ok(status) => status,
                Err(error) => SourceStatus::DeletionFailed(error),
            },
        };
        Some(SavedImage {
            path,
            bytes: output.bytes.len() as u64,
            dimensions: output.dimensions,
            quality: output.quality,
            setting: output.setting,
            source_status,
        })
    } else {
        None
    };
    Ok(FileConversion {
        source_bytes: input.len() as u64,
        source_dimensions,
        output,
    })
}

/// Read a regular image file, limiting the read to one byte beyond the codec's input limit.
pub fn read_image_file(path: &Path) -> io::Result<Vec<u8>> {
    read_bounded(&open_image(path)?)
}

fn open_image(path: &Path) -> io::Result<File> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a regular file",
        ));
    }
    File::open(path)
}

fn read_bounded(file: &File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Publish already encoded bytes in an existing directory without overwriting a file.
pub fn save_output_bytes(
    source: &Path,
    folder: &Path,
    format: OutputFormat,
    bytes: &[u8],
) -> Result<PathBuf, SaveError> {
    save_bytes(source, folder, format, bytes, false)
}

fn save_bytes(
    source: &Path,
    folder: &Path,
    format: OutputFormat,
    bytes: &[u8],
    sync: bool,
) -> Result<PathBuf, SaveError> {
    let stem = source.file_stem().ok_or(SaveError::MissingFileStem)?;
    let mut temp = NamedTempFile::new_in(folder).map_err(SaveError::Create)?;
    temp.write_all(bytes).map_err(SaveError::Write)?;
    if sync {
        temp.as_file().sync_all().map_err(SaveError::Write)?;
    }
    for index in 0..1000 {
        let mut name = OsString::from(stem);
        if index > 0 {
            name.push(format!("-{index}"));
        }
        name.push(".");
        name.push(format.extension());
        let path = folder.join(name);
        match temp.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => temp = error.file,
            Err(error) => return Err(SaveError::Publish(error.error)),
        }
    }
    Err(SaveError::NameExhausted(folder.to_owned()))
}

fn remove_source(
    path: &Path,
    original: &Metadata,
    folder: &Path,
    cancelled: &AtomicBool,
) -> Result<SourceStatus, RemoveError> {
    #[cfg(unix)]
    File::open(folder)
        .and_then(|directory| directory.sync_all())
        .map_err(RemoveError::SyncDirectory)?;
    #[cfg(not(unix))]
    let _ = folder;

    let current = fs::symlink_metadata(path).map_err(RemoveError::CheckSource)?;
    if !current.is_file()
        || !source_matches(original, &current).map_err(RemoveError::CheckSource)?
    {
        return Err(RemoveError::ChangedSource);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(SourceStatus::KeptAfterCancellation);
    }
    fs::remove_file(path).map_err(RemoveError::Delete)?;
    Ok(SourceStatus::Deleted)
}

fn source_matches(original: &Metadata, current: &Metadata) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (
            original.dev(),
            original.ino(),
            original.ctime(),
            original.ctime_nsec(),
        ) != (
            current.dev(),
            current.ino(),
            current.ctime(),
            current.ctime_nsec(),
        ) {
            return Ok(false);
        }
    }
    Ok(original.len() == current.len() && original.modified()? == current.modified()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_the_requested_folder_and_only_deletes_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("photo.png");
        image::RgbImage::from_pixel(32, 32, image::Rgb([12, 34, 56]))
            .save(&source)
            .unwrap();
        let destination = temp.path().join("converted");
        let cancelled = AtomicBool::new(false);
        let convert = |policy| {
            convert_file(
                &source,
                &destination,
                Options::default(),
                OptimizationMode::Best,
                policy,
                &cancelled,
            )
            .unwrap()
        };

        let input = fs::read(&source).unwrap();
        let kept = convert(SourcePolicy::Keep);
        assert_eq!(kept.source_bytes, input.len() as u64);
        assert_eq!(kept.source_dimensions, (32, 32));
        let kept = kept.output.unwrap();
        assert_eq!(kept.path, destination.join("photo.jxl"));
        assert!(matches!(kept.source_status, SourceStatus::Kept));
        assert_eq!(fs::read(&source).unwrap(), input);
        let first = fs::read(&kept.path).unwrap();

        let deleted = convert(SourcePolicy::DeleteAfterSave).output.unwrap();
        assert_eq!(deleted.path, destination.join("photo-1.jxl"));
        assert!(matches!(deleted.source_status, SourceStatus::Deleted));
        assert!(!source.exists());
        assert_eq!(fs::read(&kept.path).unwrap(), first);
        let saved = fs::read(&deleted.path).unwrap();
        assert_eq!(saved.len() as u64, deleted.bytes);
        assert_eq!(
            crate::decode_for_comparison(&saved).unwrap().dimensions(),
            (32, 32)
        );
    }

    #[test]
    fn failures_cancellation_and_skipped_outputs_keep_the_source() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("photo.webp");
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255]));
        let input = Setting::WebpLossless.encode(&image, &None).unwrap();
        fs::write(&source, &input).unwrap();
        let destination = temp.path().join("converted");
        let convert = |destination: &Path, format, cancelled| {
            convert_file(
                &source,
                destination,
                Options {
                    format,
                    auto_resize: false,
                },
                OptimizationMode::Best,
                SourcePolicy::DeleteAfterSave,
                &AtomicBool::new(cancelled),
            )
        };

        let skipped = convert(&destination, OutputFormat::Webp, false).unwrap();
        assert!(skipped.output.is_none());
        assert!(!destination.exists());
        assert!(matches!(
            convert(&destination, OutputFormat::JpegXl, true),
            Err(FileError::Conversion(crate::Error::Cancelled))
        ));
        assert!(!destination.exists());
        assert!(matches!(
            convert(&source, OutputFormat::JpegXl, false),
            Err(FileError::Destination(_))
        ));
        assert_eq!(fs::read(source).unwrap(), input);
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_deletion_still_returns_the_saved_output() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.png");
        let source = temp.path().join("photo.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([12, 34, 56]))
            .save(&original)
            .unwrap();
        std::os::unix::fs::symlink(&original, &source).unwrap();
        let output = convert_file(
            &source,
            temp.path(),
            Options::default(),
            OptimizationMode::Best,
            SourcePolicy::DeleteAfterSave,
            &AtomicBool::new(false),
        )
        .unwrap()
        .output
        .unwrap();

        assert!(output.path.is_file());
        assert!(matches!(
            output.source_status,
            SourceStatus::DeletionFailed(RemoveError::ChangedSource)
        ));
        assert!(source.is_symlink());
        assert!(original.is_file());
    }
}
