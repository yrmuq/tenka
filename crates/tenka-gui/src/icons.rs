use eframe::egui::{
    Align2, Atom, Button, Color32, FontId, IdSalt, Rect, Response, Role, Sense, Shape, Stroke,
    ThemePreference, Ui, Vec2, WidgetInfo, vec2,
};

pub(crate) const INFO_SIZE: Vec2 = vec2(16.0, 16.0);

pub(crate) fn globe(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let painter = ui.painter();
    let stroke = Stroke::new(1.3_f32, ui.visuals().text_color());
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
        Stroke::new(1.1_f32, foreground),
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

pub(crate) fn compare(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| icon_button(ui, label))
        .inner;
    let mut color = ui.visuals().text_color();
    if !enabled {
        color = color.gamma_multiply(0.45);
    }
    let center = response.rect.center();
    let radius = 7.5;
    let stroke = Stroke::new(1.3_f32, color);
    ui.painter()
        .with_clip_rect(Rect::from_min_max(
            center - vec2(0.0, radius + 1.0),
            center + vec2(radius + 1.0, radius + 1.0),
        ))
        .circle_filled(center, radius, color);
    ui.painter().circle_stroke(center, radius, stroke);
    response
}

pub(crate) fn errors(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let painter = ui.painter();
    painter.circle_filled(center, 9.0, ui.visuals().error_fg_color);
    let mark = ui.visuals().panel_fill;
    painter.line_segment(
        [center + vec2(0.0, -4.0), center + vec2(0.0, 1.0)],
        Stroke::new(1.7_f32, mark),
    );
    painter.circle_filled(center + vec2(0.0, 4.0), 1.0, mark);
    response
}

pub(crate) fn info(ui: &mut Ui, label: &str) {
    let (rect, response) = ui.allocate_exact_size(INFO_SIZE, Sense::hover());
    paint_info(ui, rect);
    response.on_hover_text(label);
}

pub(crate) fn about(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    paint_info(ui, response.rect);
    response
}

fn paint_info(ui: &Ui, rect: Rect) {
    let center = rect.center();
    ui.painter()
        .circle_filled(center, 7.0, ui.visuals().text_color());
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        "i",
        FontId::proportional(11.0),
        ui.visuals().panel_fill,
    );
}

pub(crate) fn share(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let left = center + vec2(-6.0, 0.0);
    let top = center + vec2(6.0, -6.0);
    let bottom = center + vec2(6.0, 6.0);
    let painter = ui.painter();
    let color = ui.visuals().text_color();
    let stroke = Stroke::new(1.3_f32, color);
    painter.line_segment([left, top], stroke);
    painter.line_segment([left, bottom], stroke);
    for node in [left, top, bottom] {
        painter.circle_filled(node, 2.5, color);
    }
    response
}

pub(crate) fn close(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let stroke = Stroke::new(1.3_f32, ui.style().interact(&response).text_color());
    for offset in [vec2(4.5, 4.5), vec2(4.5, -4.5)] {
        ui.painter()
            .line_segment([center - offset, center + offset], stroke);
    }
    response
}

pub(crate) fn right_arrow(ui: &mut Ui) {
    let height = ui.text_style_height(&eframe::egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, height), Sense::hover());
    ui.painter().arrow(
        rect.center() - vec2(5.0, 0.0),
        vec2(10.0, 0.0),
        Stroke::new(1.2_f32, ui.visuals().text_color()),
    );
}

pub(crate) fn reveal_file(ui: &mut Ui, label: &str) -> Response {
    let response = icon_button(ui, label);
    let center = response.rect.center();
    let outline = [
        (-8.0, -6.0),
        (-2.0, -6.0),
        (0.0, -3.5),
        (8.0, -3.5),
        (8.0, 6.0),
        (-8.0, 6.0),
    ]
    .map(|(x, y)| center + vec2(x, y));
    let stroke = Stroke::new(1.3_f32, ui.style().interact(&response).text_color());
    ui.painter()
        .add(Shape::closed_line(outline.to_vec(), stroke));
    response
}

pub(crate) fn output_folder(ui: &mut Ui, name: String, enabled: bool) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let arrow = IdSalt::new("folder_arrow");
        let button = Button::new((Atom::custom(arrow, vec2(10.0, 12.0)), name))
            .truncate()
            .atom_ui(ui);
        if let Some(rect) = button.rect(arrow) {
            let center = rect.center();
            let tip = center + vec2(0.0, 4.0);
            let stroke = Stroke::new(1.3_f32, ui.style().interact(&button.response).text_color());
            let painter = ui.painter();
            painter.line_segment([center - vec2(0.0, 4.0), tip], stroke);
            painter.line_segment([center + vec2(-3.5, 0.5), tip], stroke);
            painter.line_segment([center + vec2(3.5, 0.5), tip], stroke);
        }
        button.response
    })
    .inner
}

fn icon_button(ui: &mut Ui, label: &str) -> Response {
    let response = ui.add(
        Button::new("")
            .min_size(vec2(28.0, 26.0))
            .frame_when_inactive(false),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label));
    response.on_hover_text(label)
}
