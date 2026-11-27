use std::sync::mpsc::{self, Receiver};

use eframe::egui::{Color32, Context, Theme, ThemePreference, Ui};

pub(crate) const CONVERT_ACCENT: Color32 = Color32::from_rgb(242, 143, 46);
pub(crate) const ACCENT_TEXT: Color32 = Color32::from_gray(25);

pub(crate) fn apply_light_text_contrast(context: &Context) {
    context.style_mut_of(Theme::Light, |style| {
        style.visuals.override_text_color = Some(ACCENT_TEXT);
        style.visuals.weak_text_color = Some(Color32::from_gray(77));
    });
}

pub(crate) fn accent_selection(ui: &mut Ui) {
    let visuals = ui.visuals_mut();
    if visuals.dark_mode {
        visuals.override_text_color = None;
    }
    visuals.selection.bg_fill = CONVERT_ACCENT;
    visuals.selection.stroke.color = ACCENT_TEXT;
}

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
    use futures_lite::StreamExt;
    use zbus::{Connection, OwnedVariant, Proxy, block_on};

    const APPEARANCE: &str = "org.freedesktop.appearance";
    const COLOR_SCHEME: &str = "color-scheme";

    let context = context.clone();
    let (send, updates) = mpsc::channel();
    std::thread::spawn(move || {
        block_on(async move {
            let connection = Connection::session().await?;
            let proxy = Proxy::new(
                &connection,
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings",
            )
            .await?;
            let signals = proxy.receive_signal("SettingChanged").await;
            let publish = |value: &OwnedVariant| {
                if send.send(portal_theme(value)).is_err() {
                    return false;
                }
                context.request_repaint();
                true
            };
            if let Ok(value) = proxy.call("Read", &(APPEARANCE, COLOR_SCHEME)).await
                && !publish(&value)
            {
                return Ok(());
            }
            let mut signals = match signals {
                Ok(signals) => signals,
                Err(_) => proxy.receive_signal("SettingChanged").await?,
            };
            while let Some(message) = signals.next().await {
                if let Ok((namespace, key, value)) = message
                    .body()
                    .deserialize::<(String, String, OwnedVariant)>()
                    && namespace == APPEARANCE
                    && key == COLOR_SCHEME
                    && !publish(&value)
                {
                    break;
                }
            }
            Ok::<_, zbus::Error>(())
        })
    });
    updates
}

#[cfg(target_os = "linux")]
fn portal_theme(mut value: &zbus::Variant<'_>) -> ThemePreference {
    use zbus::Variant;

    while let Variant::Variant(inner) = value {
        value = inner;
    }
    match value {
        Variant::U32(1) => ThemePreference::Dark,
        // The portal's no-preference value is also the light default on some desktops.
        Variant::U32(_) => ThemePreference::Light,
        _ => ThemePreference::System,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use zbus::Variant;

    #[test]
    fn portal_schemes_and_wrapped_values_resolve_to_the_expected_theme() {
        assert_eq!(portal_theme(&Variant::U32(1)), ThemePreference::Dark);
        assert_eq!(portal_theme(&Variant::U32(2)), ThemePreference::Light);
        assert_eq!(portal_theme(&Variant::U32(0)), ThemePreference::Light);
        let wrapped = Variant::Variant(Box::new(Variant::U32(2)));
        assert_eq!(portal_theme(&wrapped), ThemePreference::Light);
    }

    #[test]
    fn selected_conversion_options_use_dark_text_on_the_orange_accent() {
        for theme in [Theme::Light, Theme::Dark] {
            let context = Context::default();
            context.set_theme(theme);
            apply_light_text_contrast(&context);
            let output = crate::tests::render_ui(&context, Default::default(), |ui| {
                accent_selection(ui);
                let _ = ui.selectable_label(true, "Selected");
            });
            assert!(output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                eframe::egui::Shape::Rect(rect) if rect.fill == CONVERT_ACCENT
            )));
            assert!(output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                eframe::egui::Shape::Text(text) if text.galley.text() == "Selected"
                    && text.override_text_color.unwrap_or(text.fallback_color) == ACCENT_TEXT
            )));
        }
    }
}
