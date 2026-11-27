use std::path::PathBuf;

use eframe::egui::Context;

pub(crate) fn take_dropped_paths(context: &Context) -> Vec<PathBuf> {
    let drops = context.input_mut(|input| std::mem::take(&mut input.raw.dropped_files));
    drops
        .into_iter()
        .map(|file| file.path().to_owned())
        .collect()
}
