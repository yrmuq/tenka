use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{APP_ICON, APP_ID};

const DESKTOP_FILE: &str = "io.github.yrmuq.tenka.desktop";
const OWNER_MARKER: &str = "X-Tenka-WindowIcon=true";

pub(crate) fn register_appimage_icon() {
    if env::var_os("APPDIR").is_none()
        || (env::var_os("WAYLAND_DISPLAY").is_none() && env::var_os("WAYLAND_SOCKET").is_none())
    {
        return;
    }
    if let Some(data) = eframe::storage_dir(APP_ID) {
        let system_dirs = env::var_os("XDG_DATA_DIRS")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let _ = register_icon(&data, env::split_paths(&system_dirs));
    }
}

fn register_icon(data: &Path, system_dirs: impl IntoIterator<Item = PathBuf>) -> io::Result<()> {
    let Some(data_home) = data.parent() else {
        return Ok(());
    };
    let applications = data_home.join("applications");
    let desktop = applications.join(DESKTOP_FILE);
    let existing = match fs::read_to_string(&desktop) {
        Ok(text) if text.lines().any(|line| line == OWNER_MARKER) => true,
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if !existing
        && system_dirs
            .into_iter()
            .any(|dir| dir.is_absolute() && dir.join("applications").join(DESKTOP_FILE).is_file())
    {
        return Ok(());
    }

    let icon = data.join("window-icon.png");
    let Some(icon_name) = icon.to_str() else {
        return Ok(());
    };
    fs::create_dir_all(data)?;
    if fs::read(&icon).as_deref().ok() != Some(APP_ICON) {
        let mut png = tempfile::NamedTempFile::new_in(data)?;
        png.write_all(APP_ICON)?;
        png.persist(&icon)?;
    }
    if !existing {
        fs::create_dir_all(&applications)?;
        let icon_name = icon_name
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        let mut entry = tempfile::NamedTempFile::new_in(&applications)?;
        writeln!(
            entry,
            "[Desktop Entry]\nType=Application\nName=Tenka\nExec=tenka\nIcon={icon_name}\nNoDisplay=true\n{OWNER_MARKER}"
        )?;
        entry.persist_noclobber(desktop)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_appimage_icon_uses_hidden_metadata_and_preserves_installed_launchers() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join(APP_ID);
        let applications = root.path().join("applications");
        let desktop = applications.join(DESKTOP_FILE);
        let icon = data.join("window-icon.png");

        register_icon(&data, []).unwrap();
        let metadata = fs::read_to_string(&desktop).unwrap();
        assert!(metadata.contains("NoDisplay=true"));
        assert!(metadata.contains(&format!("Icon={}", icon.display())));
        assert_eq!(fs::read(&icon).unwrap(), APP_ICON);

        fs::write(&icon, b"old icon").unwrap();
        register_icon(&data, []).unwrap();
        assert_eq!(fs::read(&icon).unwrap(), APP_ICON);

        fs::write(&desktop, "[Desktop Entry]\nIcon=custom\n").unwrap();
        register_icon(&data, []).unwrap();
        assert_eq!(
            fs::read_to_string(&desktop).unwrap(),
            "[Desktop Entry]\nIcon=custom\n"
        );

        let other_home = root.path().join("other");
        register_icon(&other_home.join(APP_ID), [root.path().to_owned()]).unwrap();
        assert!(!other_home.exists());
    }
}
