//! A movable in-app panel for effect settings, paint pickers, and other extended controls.

use egui::{Color32, Context, CornerRadius, Id, InnerResponse, Painter, Pos2, Rect, Sense, Shadow, Stroke, Ui, Window, pos2, vec2};

use crate::theme::Tokens;
use crate::{icons, theme};

/// Common widths used by the editor's floating property panels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatingPanelWidth {
    Compact,
    Regular,
    Wide,
    Fixed(f32),
}

impl FloatingPanelWidth {
    fn points(self) -> f32 {
        match self {
            Self::Compact => 240.0,
            Self::Regular => 304.0,
            Self::Wide => 400.0,
            Self::Fixed(width) => {
                if width.is_finite() {
                    width.clamp(120.0, 1_024.0)
                } else {
                    240.0
                }
            }
        }
    }
}

/// Fixed heights keep Pencil-based layouts exact; Hug follows the contents.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FloatingPanelHeight {
    #[default]
    Hug,
    Fixed(f32),
}

/// The caller owns visibility and contents. A dragged position lasts only until the panel closes.
pub struct FloatingPanel<'a> {
    id: Id,
    initial_pos: Pos2,
    width: FloatingPanelWidth,
    height: FloatingPanelHeight,
    header_height: f32,
    fill: Option<Color32>,
    radius: u8,
    shadow: Option<Shadow>,
    properties_anchor: bool,
    open: Option<&'a mut bool>,
}

impl<'a> FloatingPanel<'a> {
    /// Shared dark surface for every floating property panel.
    pub const DARK_SURFACE: Color32 = Color32::from_rgb(44, 44, 44);
    pub const DARK_FIELD: Color32 = Color32::from_rgb(56, 56, 56);

    pub fn surface(t: Tokens) -> Color32 {
        if t.dark { Self::DARK_SURFACE } else { t.panel }
    }

    fn open_panels_id() -> Id {
        Id::new("selection_floating_panels")
    }

    fn visible_panels_id() -> Id {
        Id::new("visible_floating_panels")
    }

    fn shown_panels_id() -> Id {
        Id::new("shown_floating_panels")
    }

    fn previous_panels_id() -> Id {
        Id::new("previous_floating_panels")
    }

    fn properties_edge_id() -> Id {
        Id::new("floating_panel_properties_edge")
    }

    fn workspace_bounds_id() -> Id {
        Id::new("floating_panel_workspace_bounds")
    }

    pub fn begin_visibility_tracking(ctx: &Context) {
        ctx.data_mut(|data| {
            let shown = data.remove_temp::<Vec<Id>>(Self::shown_panels_id()).unwrap_or_default();
            data.insert_temp(Self::previous_panels_id(), shown);
            data.insert_temp(Self::shown_panels_id(), Vec::<Id>::new());
            data.insert_temp(Self::visible_panels_id(), Vec::<Rect>::new());
            data.remove_temp::<f32>(Self::properties_edge_id());
            data.insert_temp(Self::workspace_bounds_id(), None::<Rect>);
        });
    }

    /// Record the actual left edge after the resizable properties sidebar is laid out.
    pub fn properties_edge(ctx: &Context, left: f32) {
        ctx.data_mut(|data| data.insert_temp(Self::properties_edge_id(), left));
    }

    /// The canvas area below the document tabs and between the sidebars.
    pub fn workspace_bounds(ctx: &Context, bounds: Rect) {
        ctx.data_mut(|data| data.insert_temp(Self::workspace_bounds_id(), Some(bounds)));
    }

    pub fn visible_panel_contains(ctx: &Context, position: Pos2) -> Option<bool> {
        ctx.data(|data| data.get_temp::<Vec<Rect>>(Self::visible_panels_id()))
            .filter(|rects| !rects.is_empty())
            .map(|rects| rects.iter().any(|rect| rect.contains(position)))
    }

    /// Toggle a property panel from its trigger without coupling the trigger to
    /// egui's selection-menu popup state.
    pub fn toggle(ctx: &Context, id: Id, clicked: bool) -> bool {
        let state_id = id.with("open");
        let was_open = ctx.data(|data| data.get_temp::<bool>(state_id)).unwrap_or(false);
        let open = if clicked { !was_open } else { was_open };
        if !open && !clicked {
            return false;
        }
        ctx.data_mut(|data| {
            if clicked {
                data.insert_temp(state_id, open);
                if open {
                    data.remove_temp::<Pos2>(id.with("position"));
                }
            }
            let mut ids = data.get_temp::<Vec<Id>>(Self::open_panels_id()).unwrap_or_default();
            if open && !ids.contains(&id) {
                ids.push(id);
            } else if !open && clicked {
                ids.retain(|candidate| *candidate != id);
            }
            data.insert_temp(Self::open_panels_id(), ids);
        });
        open
    }

    pub fn dismiss(ctx: &Context, id: Id) {
        ctx.data_mut(|data| {
            data.insert_temp(id.with("open"), false);
            data.remove_temp::<Pos2>(id.with("position"));
            let mut ids = data.get_temp::<Vec<Id>>(Self::open_panels_id()).unwrap_or_default();
            ids.retain(|candidate| *candidate != id);
            data.insert_temp(Self::open_panels_id(), ids);
        });
    }

    /// A property panel must not retain its open state after its layer selection changes.
    pub fn dismiss_selection_panels(ctx: &Context) {
        ctx.data_mut(|data| {
            for id in data.remove_temp::<Vec<Id>>(Self::open_panels_id()).unwrap_or_default() {
                data.insert_temp(id.with("open"), false);
                data.remove_temp::<Pos2>(id.with("position"));
            }
        });
    }

    /// A simple titled header. Callers may paint any other header instead.
    pub fn title_header(ui: &mut Ui, rect: Rect, title: &str, close_id: Id) -> bool {
        let t = Tokens::get(ui.ctx());
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, t.border));
        ui.painter().text(pos2(rect.left() + 16.0, rect.center().y), egui::Align2::LEFT_CENTER, title, theme::semibold(11.0), t.text);
        let close = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(24.0, 24.0));
        Self::close_button(ui, close_id, close, |painter, rect| {
            icons::paint(painter, rect.shrink(6.0), icons::Icon::Close, t.icon);
        })
    }

    /// The current dragged position, or the initial anchor before the first drag.
    pub fn position(ctx: &Context, id: Id, initial_pos: Pos2) -> Pos2 {
        ctx.data(|d| d.get_temp::<Pos2>(id.with("position"))).unwrap_or(initial_pos)
    }

    pub fn new(id: Id, initial_pos: Pos2) -> Self {
        Self {
            id,
            initial_pos,
            width: FloatingPanelWidth::Compact,
            height: FloatingPanelHeight::Hug,
            header_height: 40.0,
            fill: None,
            radius: 13,
            shadow: None,
            properties_anchor: false,
            open: None,
        }
    }

    pub fn width(mut self, width: FloatingPanelWidth) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: FloatingPanelHeight) -> Self {
        self.height = height;
        self
    }

    pub fn header_height(mut self, height: f32) -> Self {
        self.header_height = if height.is_finite() { height.clamp(0.0, 120.0) } else { 40.0 };
        self
    }

    pub fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self
    }

    pub fn corner_radius(mut self, radius: u8) -> Self {
        self.radius = radius;
        self
    }

    pub fn shadow(mut self, shadow: Shadow) -> Self {
        self.shadow = Some(shadow);
        self
    }

    /// Anchor a panel opened from the properties sidebar even when another panel is visible.
    pub fn anchor_to_properties(mut self) -> Self {
        self.properties_anchor = true;
        self
    }

    pub fn open(mut self, open: &'a mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Reserve the draggable header, then let the caller compose its own title,
    /// tabs, actions, and body. The same surface and drag behavior are used for
    /// every header layout.
    pub fn show_with_header<R>(self, ctx: &Context, content: impl FnOnce(&mut Ui, Rect) -> R) -> Option<InnerResponse<Option<R>>> {
        let width = self.width.points();
        let header_height = self.header_height;
        self.show(ctx, |ui| {
            let spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let (_, header) = ui.allocate_space(vec2(width, header_height));
            ui.spacing_mut().item_spacing = spacing;
            content(ui, header)
        })
    }

    /// A close action shared by panel headers. The icon remains caller-owned so
    /// each panel can retain the exact SVG used in its design.
    pub fn close_button(ui: &mut Ui, id: Id, rect: Rect, paint_icon: impl FnOnce(&Painter, Rect)) -> bool {
        let response = ui.interact(rect, id, Sense::click());
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, CornerRadius::same(5), Tokens::get(ui.ctx()).hover);
        }
        paint_icon(ui.painter(), rect);
        response.clicked()
    }

    pub fn show<R>(self, ctx: &Context, content: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<Option<R>>> {
        let t = Tokens::get(ctx);
        let width = self.width.points();
        let known_height = ctx.data(|d| d.get_temp::<f32>(self.id.with("height"))).unwrap_or(300.0);
        let height = match self.height {
            FloatingPanelHeight::Hug => known_height,
            FloatingPanelHeight::Fixed(height) => {
                if height.is_finite() {
                    height.clamp(40.0, 2_048.0)
                } else {
                    300.0
                }
            }
        };
        let bounds = ctx.data(|data| data.get_temp::<Option<Rect>>(Self::workspace_bounds_id())).flatten().unwrap_or_else(|| ctx.content_rect());
        let was_shown = ctx.data(|data| {
            [Self::previous_panels_id(), Self::shown_panels_id()]
                .iter()
                .any(|key| data.get_temp::<Vec<Id>>(*key).is_some_and(|ids| ids.contains(&self.id)))
        });
        if !was_shown {
            let edge = ctx.data(|data| data.get_temp::<f32>(Self::properties_edge_id()));
            let click = ctx.input(|input| input.pointer.any_click().then(|| input.pointer.interact_pos()).flatten());
            let initial = match (edge, click) {
                (Some(edge), Some(click)) if click.x >= edge && click.x <= ctx.content_rect().right() => {
                    // The window frame adds one point of stroke on each side.
                    let x = edge - width - 2.0 - 12.0;
                    let y = click.y - height * 0.5;
                    pos2(x, y)
                }
                (Some(edge), _) if self.properties_anchor => pos2(edge - width - 2.0 - 12.0, self.initial_pos.y),
                _ => self.initial_pos,
            };
            ctx.data_mut(|data| data.insert_temp(self.id.with("position"), initial));
        }
        let stored = Self::position(ctx, self.id, self.initial_pos);
        let x = if stored.x.is_finite() { stored.x } else { bounds.left() + 8.0 };
        let y = if stored.y.is_finite() { stored.y } else { bounds.top() + 8.0 };
        let pos = pos2(
            x.clamp(bounds.left() + 8.0, (bounds.right() - width - 8.0).max(bounds.left() + 8.0)),
            y.clamp(bounds.top() + 8.0, (bounds.bottom() - height - 8.0).max(bounds.top() + 8.0)),
        );
        if pos != stored {
            ctx.data_mut(|data| data.insert_temp(self.id.with("position"), pos));
        }
        let shadow =
            self.shadow.unwrap_or(Shadow { offset: [0, 10], blur: 14, spread: 0, color: Color32::from_black_alpha(if t.dark { 89 } else { 45 }) });
        let frame = egui::Frame::window(&ctx.global_style())
            .fill(self.fill.unwrap_or(Self::surface(t)))
            .corner_radius(CornerRadius::same(self.radius))
            .stroke(Stroke::new(1.0, t.border))
            .shadow(shadow)
            .inner_margin(egui::Margin::ZERO);
        let mut window =
            Window::new("Floating panel").id(self.id).title_bar(false).fixed_pos(pos).resizable(false).order(egui::Order::Foreground).frame(frame);
        window = match self.height {
            FloatingPanelHeight::Hug => window.auto_sized(),
            FloatingPanelHeight::Fixed(_) => window.fixed_size(vec2(width, height)),
        };
        if let Some(open) = self.open {
            window = window.open(open);
        }
        let result = window.show(ctx, |ui| {
            let header = egui::Rect::from_min_size(ui.cursor().min, vec2(width, self.header_height));
            let drag = ui.interact(header, self.id.with("drag"), Sense::drag());
            if drag.dragged() {
                let delta = ui.input(|input| input.pointer.delta());
                if delta.is_finite() {
                    ctx.data_mut(|d| d.insert_temp(self.id.with("position"), pos + delta));
                    ctx.request_repaint();
                }
            }
            ui.set_width(width);
            if matches!(self.height, FloatingPanelHeight::Fixed(_)) {
                ui.set_min_height(height);
            }
            content(ui)
        });
        if let Some(result) = &result {
            if !was_shown {
                ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, self.id));
            }
            ctx.data_mut(|data| {
                data.insert_temp(self.id.with("height"), result.response.rect.height());
                let mut shown = data.get_temp::<Vec<Id>>(Self::shown_panels_id()).unwrap_or_default();
                if !shown.contains(&self.id) {
                    shown.push(self.id);
                }
                data.insert_temp(Self::shown_panels_id(), shown);
                if let Some(mut rects) = data.get_temp::<Vec<Rect>>(Self::visible_panels_id()) {
                    rects.push(result.response.rect);
                    data.insert_temp(Self::visible_panels_id(), rects);
                }
            });
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput};

    fn frame(ctx: &Context, time: f64, events: Vec<Event>, show: bool) {
        let input =
            RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1_200.0, 800.0))), time: Some(time), events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            FloatingPanel::begin_visibility_tracking(ui.ctx());
            FloatingPanel::properties_edge(ui.ctx(), 1_000.0);
            FloatingPanel::workspace_bounds(ui.ctx(), Rect::from_min_max(pos2(100.0, 80.0), pos2(1_000.0, 700.0)));
            if show {
                FloatingPanel::new(Id::new("test_panel"), pos2(100.0, 100.0)).height(FloatingPanelHeight::Fixed(200.0)).show_with_header(
                    ui.ctx(),
                    |ui, _| {
                        ui.allocate_exact_size(vec2(240.0, 160.0), Sense::hover());
                    },
                );
            }
        });
        output.textures_delta.clear();
    }

    fn button(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    #[test]
    fn sidebar_panel_drags_and_reanchors_after_reopening() {
        let ctx = Context::default();
        let id = Id::new("test_panel");
        let first_trigger = pos2(1_100.0, 300.0);
        frame(&ctx, 0.01, vec![Event::PointerMoved(first_trigger), button(first_trigger, true)], false);
        frame(&ctx, 0.02, vec![button(first_trigger, false)], true);
        let first = FloatingPanel::position(&ctx, id, Pos2::ZERO);
        assert_eq!(first, pos2(746.0, 200.0));

        let header = pos2(800.0, 220.0);
        frame(&ctx, 0.03, vec![Event::PointerMoved(header)], true);
        frame(&ctx, 0.04, vec![button(header, true)], true);
        frame(&ctx, 0.05, vec![Event::PointerMoved(pos2(850.0, 260.0))], true);
        let dragged = FloatingPanel::position(&ctx, id, Pos2::ZERO);
        assert!(dragged.x > first.x && dragged.y > first.y, "the header should drag the panel");
        frame(&ctx, 0.06, vec![button(pos2(850.0, 260.0), false)], true);
        frame(&ctx, 0.07, Vec::new(), false);

        let next_trigger = pos2(1_100.0, 500.0);
        frame(&ctx, 0.08, vec![Event::PointerMoved(next_trigger), button(next_trigger, true)], false);
        frame(&ctx, 0.09, vec![button(next_trigger, false)], true);
        assert_eq!(FloatingPanel::position(&ctx, id, Pos2::ZERO), pos2(746.0, 400.0));

        frame(&ctx, 0.10, Vec::new(), false);
        let top_trigger = pos2(1_100.0, 100.0);
        frame(&ctx, 0.11, vec![Event::PointerMoved(top_trigger), button(top_trigger, true)], false);
        frame(&ctx, 0.12, vec![button(top_trigger, false)], true);
        assert_eq!(FloatingPanel::position(&ctx, id, Pos2::ZERO), pos2(746.0, 88.0));
        assert!(ctx.memory(|memory| memory.area_rect(id)).is_some_and(|rect| rect.top() >= 88.0));
    }
}
