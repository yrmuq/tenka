use std::{
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use serde_json::{Value, json};
use tenka_core::{
    Error as CodecError, OptimizationMode, Options, OutputFormat,
    files::{
        FileConversion, FileError, FolderError, FolderProgress, FolderReport, ImageResult,
        RemoveError, SaveError, SourcePolicy, SourceStatus, SubfolderError, convert_file,
        convert_folder,
    },
};

use crate::jobs::codec_error_label;

pub fn print_help() {
    println!(
        "Usage: tenka convert <file-or-folder> --output <directory> [options]\n\
         \n\
         Options:\n\
           --format <jxl|webp>          Output format (default: jxl)\n\
           --mode <best|balance|cheap>  Compression mode (default: best)\n\
           --delete-source              Delete each source after saving its result\n\
           --help                       Show this help\n\
         A final JSON report is written to stdout. Progress is written as JSON lines to stderr."
    );
}

struct Command {
    source: PathBuf,
    output: PathBuf,
    options: Options,
    mode: OptimizationMode,
    source_policy: SourcePolicy,
}

impl Command {
    fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Option<Self>, String> {
        let mut source = None;
        let mut output = None;
        let mut options = Options::default();
        let mut mode = OptimizationMode::default();
        let mut source_policy = SourcePolicy::Keep;
        while let Some(argument) = args.next() {
            match argument.to_str() {
                Some("--help" | "-h") => return Ok(None),
                Some("--output") => {
                    output = Some(PathBuf::from(
                        args.next()
                            .filter(|path| !path.is_empty())
                            .filter(|path| !path.to_string_lossy().starts_with('-'))
                            .ok_or("Expected a directory after --output")?,
                    ));
                }
                Some("--format") => {
                    options.format = match args.next().as_deref().and_then(|value| value.to_str()) {
                        Some("jxl") => OutputFormat::JpegXl,
                        Some("webp") => OutputFormat::Webp,
                        _ => return Err("Expected jxl or webp after --format".into()),
                    };
                }
                Some("--mode") => {
                    mode = match args.next().as_deref().and_then(|value| value.to_str()) {
                        Some("best") => OptimizationMode::Best,
                        Some("balance") => OptimizationMode::Balance,
                        Some("cheap") => OptimizationMode::Cheap,
                        _ => return Err("Expected best, balance, or cheap after --mode".into()),
                    };
                }
                Some("--delete-source") => source_policy = SourcePolicy::DeleteAfterSave,
                Some(flag) if flag.starts_with('-') => {
                    return Err(format!("Unknown option {flag}"));
                }
                _ if source.is_none() && !argument.is_empty() => {
                    source = Some(PathBuf::from(argument));
                }
                _ => return Err("Expected one source file or folder".into()),
            }
        }
        Ok(Some(Self {
            source: source.ok_or("Expected a source file or folder")?,
            output: output.ok_or("Expected --output <directory>")?,
            options,
            mode,
            source_policy,
        }))
    }

    fn convert(
        &self,
        cancelled: &AtomicBool,
        mut progress: impl FnMut(FolderProgress<'_>),
    ) -> Result<FolderReport, Value> {
        let metadata = fs::metadata(&self.source)
            .map_err(|error| problem("open_source", error.to_string()))?;
        if metadata.is_dir() {
            return convert_folder(
                &self.source,
                &self.output,
                self.options,
                self.mode,
                self.source_policy,
                cancelled,
                progress,
            )
            .map_err(|error| {
                let code = match error {
                    FolderError::Source(_) => "open_source_folder",
                    FolderError::Destination(_) => "open_output_folder",
                };
                problem(code, error_message(&error))
            });
        }
        if !metadata.is_file() {
            return Err(problem(
                "invalid_source",
                "Expected a regular file or directory",
            ));
        }
        let started = Instant::now();
        progress(FolderProgress::Scanned { total: 1 });
        progress(FolderProgress::Converting {
            source: &self.source,
            completed: 0,
            total: 1,
        });
        let result = convert_file(
            &self.source,
            &self.output,
            self.options,
            self.mode,
            self.source_policy,
            cancelled,
        );
        let interrupted = matches!(result, Err(FileError::Conversion(CodecError::Cancelled)));
        let mut images = Vec::new();
        if !interrupted {
            let image = ImageResult {
                source: self.source.clone(),
                result,
            };
            progress(FolderProgress::FinishedFile {
                image: &image,
                completed: 1,
                total: 1,
            });
            images.push(image);
        }
        Ok(FolderReport {
            total: 1,
            scan_errors: Vec::new(),
            images,
            elapsed: started.elapsed(),
            cancelled: interrupted || cancelled.load(Ordering::Relaxed),
        })
    }
}

static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn run(args: impl Iterator<Item = OsString>) -> i32 {
    let (code, value) = match Command::parse(args) {
        Ok(Some(command)) => {
            match ctrlc::set_handler(|| CANCELLED.store(true, Ordering::Relaxed)) {
                Ok(()) => run_conversion(command),
                Err(error) => (1, fatal(problem("signal_handler", error.to_string()))),
            }
        }
        Ok(None) => {
            print_help();
            return 0;
        }
        Err(error) => (2, fatal(problem("invalid_arguments", error))),
    };
    if write_json(&mut io::stdout().lock(), &value).is_err() {
        return 1;
    }
    code
}

fn run_conversion(command: Command) -> (i32, Value) {
    let mut stderr = io::stderr().lock();
    let report = command.convert(&CANCELLED, |event| {
        let _ = write_json(&mut stderr, &progress_json(event));
    });
    let report = match report {
        Ok(report) => report,
        Err(error) => return (1, fatal(error)),
    };
    let mut converted = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut deletion_failed = 0;
    let mut source_bytes = 0;
    let mut output_bytes = 0;
    let mut files = Vec::with_capacity(report.images.len());
    for image in report.images {
        let source = image.source.to_string_lossy();
        match image.result {
            Ok(FileConversion {
                output: Some(output),
                source_bytes: input_bytes,
                source_dimensions,
            }) => {
                converted += 1;
                source_bytes += input_bytes;
                output_bytes += output.bytes;
                let (source_status, source_error) = match output.source_status {
                    SourceStatus::Kept => ("kept", None),
                    SourceStatus::Deleted => ("deleted", None),
                    SourceStatus::KeptAfterCancellation => ("kept_after_cancellation", None),
                    SourceStatus::DeletionFailed(error) => {
                        deletion_failed += 1;
                        let code = match error {
                            RemoveError::SyncDirectory(_) => "sync_output_directory",
                            RemoveError::CheckSource(_) => "check_source_before_deletion",
                            RemoveError::ChangedSource => "source_changed",
                            RemoveError::Delete(_) => "delete_source",
                        };
                        (
                            "deletion_failed",
                            Some(problem(code, error_message(&error))),
                        )
                    }
                };
                files.push(json!({
                    "source": source, "status": "converted",
                    "output": output.path.to_string_lossy(),
                    "source_bytes": input_bytes, "output_bytes": output.bytes,
                    "source_dimensions": source_dimensions, "output_dimensions": output.dimensions,
                    "quality": output.quality, "source_status": source_status,
                    "source_error": source_error,
                }));
            }
            Ok(image) => {
                skipped += 1;
                files.push(json!({
                    "source": source, "status": "skipped", "source_bytes": image.source_bytes,
                    "source_dimensions": image.source_dimensions, "source_status": "kept",
                }));
            }
            Err(error) => {
                failed += 1;
                files.push(json!({
                    "source": source, "status": "failed", "error": file_error(&error),
                }));
            }
        }
    }
    let scan_errors: Vec<_> = report
        .scan_errors
        .into_iter()
        .map(|error| {
            json!({
                "path": error.path.to_string_lossy(),
                "error": problem("read_folder_entry", error.error.to_string()),
            })
        })
        .collect();
    let (code, status) = if report.cancelled {
        (130, "cancelled")
    } else if failed + deletion_failed + scan_errors.len() > 0 {
        (1, "failed")
    } else {
        (0, "completed")
    };
    (
        code,
        json!({
            "status": status,
            "source": command.source.to_string_lossy(), "destination": command.output.to_string_lossy(),
            "elapsed_ms": report.elapsed.as_millis() as u64,
            "total": report.total, "converted": converted, "skipped": skipped, "failed": failed,
            "deletion_failed": deletion_failed, "remaining": report.total - files.len(),
            "source_bytes": source_bytes, "output_bytes": output_bytes,
            "files": files, "scan_errors": scan_errors,
        }),
    )
}

fn progress_json(event: FolderProgress<'_>) -> Value {
    match event {
        FolderProgress::Scanning => json!({"event": "scanning"}),
        FolderProgress::Scanned { total } => json!({"event": "scanned", "total": total}),
        FolderProgress::Converting {
            source,
            completed,
            total,
        } => json!({
            "event": "converting", "source": source.to_string_lossy(),
            "completed": completed, "total": total,
        }),
        FolderProgress::FinishedFile {
            image,
            completed,
            total,
        } => {
            let status = match &image.result {
                Ok(result) if result.output.is_some() => "converted",
                Ok(_) => "skipped",
                Err(_) => "failed",
            };
            json!({
                "event": "finished_file", "source": image.source.to_string_lossy(),
                "status": status, "completed": completed, "total": total,
            })
        }
    }
}

fn file_error(error: &FileError) -> Value {
    let code = match error {
        FileError::Conversion(error) => {
            return problem(
                format!("E{}", codec_error_label(error)),
                error_message(error),
            );
        }
        FileError::Read(_) => "read_source",
        FileError::Destination(error) => match error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<SubfolderError>())
        {
            Some(SubfolderError::InvalidRelativePath(_)) => "invalid_relative_path",
            Some(SubfolderError::Create { .. }) => "create_output_subfolder",
            None => "create_output_folder",
        },
        FileError::Save(error) => match error {
            SaveError::MissingFileStem => "missing_file_stem",
            SaveError::Create(_) => "create_temp_output",
            SaveError::Write(_) => "write_output",
            SaveError::Publish(_) => "publish_output",
            SaveError::NameExhausted(_) => "output_names_exhausted",
        },
    };
    problem(code, error_message(error))
}

fn error_message(error: &dyn Error) -> String {
    if let Some(cause) = error.source() {
        format!("{error}: {cause}")
    } else {
        error.to_string()
    }
}

fn fatal(error: Value) -> Value {
    json!({"status": "failed", "error": error})
}

fn problem(code: impl Into<String>, message: impl Into<String>) -> Value {
    json!({"code": code.into(), "message": message.into()})
}

fn write_json(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
