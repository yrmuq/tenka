use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

use eframe::egui::Context;

pub(crate) struct FileDrops(Receiver<Vec<PathBuf>>);

impl FileDrops {
    pub(crate) fn new(context: &Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        let context = context.clone();
        #[cfg(target_os = "linux")]
        smithay_clipboard::set_file_drop_handler(move |paths| {
            if sender.send(paths).is_ok() {
                context.request_repaint();
            }
        });
        #[cfg(not(target_os = "linux"))]
        let _ = (sender, context);
        Self(receiver)
    }

    pub(crate) fn take(&self, context: &Context) -> Vec<PathBuf> {
        let drops = context.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
        drops
            .into_iter()
            .filter_map(|file| file.path)
            .chain(self.0.try_iter().flatten())
            .collect()
    }
}
