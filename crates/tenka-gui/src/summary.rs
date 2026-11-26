use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::jobs::{Conversion, Failure};

#[derive(Default)]
pub(crate) struct Summary {
    pub converted: usize,
    pub skipped: usize,
    pub errors: usize,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub elapsed: Duration,
    output_folder: Option<PathBuf>,
}

impl Summary {
    pub fn record(&mut self, result: &Result<Option<Conversion>, Failure>) {
        match result {
            Ok(Some(output)) => {
                let folder = std::path::absolute(&output.path)
                    .ok()
                    .and_then(|path| path.parent().map(Path::to_path_buf));
                if self.converted == 0 {
                    self.output_folder = folder;
                } else if folder != self.output_folder {
                    self.output_folder = None;
                }
                self.converted += 1;
                self.source_bytes += output.source_bytes as u64;
                self.output_bytes += output.bytes as u64;
            }
            Ok(None) => self.skipped += 1,
            Err(_) => self.errors += 1,
        }
    }

    pub fn attempted(&self) -> usize {
        self.converted + self.skipped + self.errors
    }

    pub fn folder(&self) -> Option<&Path> {
        self.output_folder.as_deref()
    }
}
