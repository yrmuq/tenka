use eframe::egui::{
    Align2, Button, Color32, FontId, Response, Shape, Stroke, ThemePreference, Ui, WidgetInfo,
    WidgetType, vec2,
};

pub(crate) fn globe(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let painter = ui.painter();
    let stroke = Stroke::new(1.3, ui.visuals().text_color());
    painter.circle_stroke(center, 8.0, stroke);
    painter.line_segment([center - vec2(8.0, 0.0), center + vec2(8.0, 0.0)], stroke);
    let meridians = (0..24)
        .map(|step| {
            let angle = step as f32 * std::f32::consts::TAU / 24.0;
            center + vec2(3.7 * angle.cos(), 8.0 * angle.sin())
        })
        .collect();
    painter.add(Shape::closed_line(meridians, stroke));
    response
}

pub(crate) fn theme(ui: &mut Ui, preference: ThemePreference, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let painter = ui.painter();
    let foreground = ui.visuals().text_color();
    let fill = match preference {
        ThemePreference::System => foreground,
        ThemePreference::Light => Color32::from_gray(248),
        ThemePreference::Dark => Color32::from_gray(30),
    };
    let position = |(x, y)| center + vec2(x, y);
    let outline = [
        (-5.0, -8.0),
        (5.0, -8.0),
        (10.0, -4.0),
        (8.0, 1.0),
        (5.0, -1.0),
        (5.0, 8.0),
        (-5.0, 8.0),
        (-5.0, -1.0),
        (-8.0, 1.0),
        (-10.0, -4.0),
    ]
    .map(position);
    for corners in [[0, 1, 5, 6], [1, 2, 3, 4], [7, 8, 9, 0]] {
        painter.add(Shape::convex_polygon(
            corners.map(|index| outline[index]).to_vec(),
            fill,
            Stroke::NONE,
        ));
    }
    painter.add(Shape::closed_line(
        outline.to_vec(),
        Stroke::new(1.1, foreground),
    ));
    if preference == ThemePreference::System {
        painter.text(
            center + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            "A",
            FontId::proportional(9.0),
            ui.visuals().panel_fill,
        );
    }
    response
}

fn icon_button(ui: &mut Ui, label: &str) -> Response {
    let response = ui.add(
        Button::new("")
            .min_size(vec2(28.0, 26.0))
            .frame_when_inactive(false),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response.on_hover_text(label)
}
