use std::io::{ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::OnceLock;

use sctk::data_device_manager::data_device::DataDeviceData;
use sctk::data_device_manager::data_offer::DragOffer;
use sctk::reexports::calloop::{LoopHandle, PostAction};
use sctk::reexports::client::Proxy;
use sctk::reexports::client::protocol::wl_data_device::WlDataDevice;
use sctk::reexports::client::protocol::wl_data_device_manager::DndAction;

use crate::state::{State, set_non_blocking};

type FileDropHandler = Box<dyn Fn(Vec<PathBuf>) + Send + Sync>;

static HANDLER: OnceLock<FileDropHandler> = OnceLock::new();
const URI_LIST: &str = "text/uri-list";
const MAX_URI_LIST_BYTES: usize = 16 * 1024 * 1024;

/// Register a process-wide receiver for local files dropped onto a Wayland window.
pub fn set_file_drop_handler(handler: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) {
    let _ = HANDLER.set(Box::new(handler));
}

pub(crate) fn enter(device: &WlDataDevice) {
    if let Some(offer) = file_offer(device).filter(|_| HANDLER.get().is_some()) {
        offer.accept_mime_type(offer.serial, Some(URI_LIST.into()));
        // Decline Move so the source keeps the original files.
        offer.set_actions(DndAction::Copy, DndAction::Copy);
    }
}

pub(crate) fn receive(device: &WlDataDevice, loop_handle: &LoopHandle<'static, State>) {
    let (Some(handler), Some(offer)) = (HANDLER.get(), file_offer(device)) else {
        return;
    };
    if offer.inner().version() >= 3 && offer.selected_action != DndAction::Copy {
        offer.destroy();
        return;
    }
    let Ok(pipe) = offer.receive(URI_LIST.into()) else {
        offer.destroy();
        return;
    };
    if set_non_blocking(pipe.as_raw_fd()).is_err() {
        offer.destroy();
        return;
    }

    let transfer = offer.clone();
    let mut content = Vec::new();
    let mut buffer = [0; 8192];
    let source = loop_handle.insert_source(pipe, move |_, pipe, _| {
        // SAFETY: The registered file descriptor remains open. We only read from it.
        let pipe = unsafe { pipe.get_mut() };
        match pipe.read(&mut buffer) {
            Ok(0) => {
                let paths = local_paths(&content);
                if !paths.is_empty() {
                    if transfer.inner().is_alive() {
                        transfer.finish();
                    }
                    handler(paths);
                }
            }
            Ok(count) if content.len() + count <= MAX_URI_LIST_BYTES => {
                content.extend_from_slice(&buffer[..count]);
                return PostAction::Continue;
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                return PostAction::Continue;
            }
            _ => {}
        }
        if transfer.inner().is_alive() {
            transfer.destroy();
        }
        PostAction::Remove
    });
    if source.is_err() {
        offer.destroy();
    }
}

fn file_offer(device: &WlDataDevice) -> Option<DragOffer> {
    let offer = device.data::<DataDeviceData>()?.drag_offer()?;
    offer.with_mime_types(|types| types.iter().any(|mime| mime == URI_LIST))
        .then_some(offer)
}

fn local_paths(content: &[u8]) -> Vec<PathBuf> {
    let Ok(list) = std::str::from_utf8(content) else {
        return Vec::new();
    };
    list.lines()
        .map(str::trim)
        .filter_map(|line| url::Url::parse(line).ok())
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}
