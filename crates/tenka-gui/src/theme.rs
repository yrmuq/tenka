use std::sync::mpsc::{self, Receiver};

use eframe::egui::{Context, ThemePreference};

pub(crate) struct SystemTheme {
    updates: Receiver<ThemePreference>,
    system: Option<ThemePreference>,
    applied: Option<ThemePreference>,
}

impl SystemTheme {
    pub(crate) fn new(context: &Context) -> Self {
        Self {
            updates: watch_system_theme(context),
            system: None,
            applied: None,
        }
    }

    pub(crate) fn apply(&mut self, context: &Context, preference: ThemePreference) {
        if let Some(system) = self.updates.try_iter().last() {
            self.system = Some(system);
        }
        let effective = match preference {
            ThemePreference::System => self.system.unwrap_or(preference),
            _ => preference,
        };
        if self.applied != Some(effective) {
            context.set_theme(effective);
            self.applied = Some(effective);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn watch_system_theme(_: &Context) -> Receiver<ThemePreference> {
    mpsc::channel().1
}

#[cfg(target_os = "linux")]
fn watch_system_theme(context: &Context) -> Receiver<ThemePreference> {
    use zbus::{
        blocking::{Connection, Proxy},
        zvariant::OwnedValue,
    };

    const APPEARANCE: &str = "org.freedesktop.appearance";
    const COLOR_SCHEME: &str = "color-scheme";

    let context = context.clone();
    let (send, updates) = mpsc::channel();
    std::thread::spawn(move || -> zbus::Result<()> {
        let connection = Connection::session()?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        let signals = proxy.receive_signal("SettingChanged");
        let publish = |value: &OwnedValue| {
            if send.send(portal_theme(value)).is_err() {
                return false;
            }
            context.request_repaint();
            true
        };
        if let Ok(value) = proxy.call("Read", &(APPEARANCE, COLOR_SCHEME))
            && !publish(&value)
        {
            return Ok(());
        }
        for message in signals.or_else(|_| proxy.receive_signal("SettingChanged"))? {
            if let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
                && namespace == APPEARANCE
                && key == COLOR_SCHEME
                && !publish(&value)
            {
                break;
            }
        }
        Ok(())
    });
    updates
}

#[cfg(target_os = "linux")]
fn portal_theme(mut value: &zbus::zvariant::Value<'_>) -> ThemePreference {
    use zbus::zvariant::Value;

    while let Value::Value(inner) = value {
        value = inner;
    }
    match value {
        Value::U32(1) => ThemePreference::Dark,
        // The portal's no-preference value is also the light default on some desktops.
        Value::U32(_) => ThemePreference::Light,
        _ => ThemePreference::System,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn portal_schemes_and_wrapped_values_resolve_to_the_expected_theme() {
        assert_eq!(portal_theme(&Value::U32(1)), ThemePreference::Dark);
        assert_eq!(portal_theme(&Value::U32(2)), ThemePreference::Light);
        assert_eq!(portal_theme(&Value::U32(0)), ThemePreference::Light);
        let wrapped = Value::Value(Box::new(Value::U32(2)));
        assert_eq!(portal_theme(&wrapped), ThemePreference::Light);
    }
}
