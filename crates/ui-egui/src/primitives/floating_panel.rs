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

/// The caller owns visibility and contents. The panel persists its dragged position per `id`.
pub struct FloatingPanel<'a> {
    id: Id,
    initial_pos: Pos2,
    width: FloatingPanelWidth,
    height: FloatingPanelHeight,
    header_height: f32,
    fill: Option<Color32>,
    radius: u8,
    shadow: Option<Shadow>,
    open: Option<&'a mut bool>,
}

impl<'a> FloatingPanel<'a> {
    /// Shared dark surface for every floating property panel.
    pub const DARK_SURFACE: Color32 = Color32::from_rgb(44, 44, 44);
    pub const DARK_FIELD: Color32 = Color32::from_rgb(56, 56, 56);

    pub fn surface(t: Tokens) -> Color32 {
        if t.dark { Self::DARK_SURFACE } else { t.panel }
    }

    /// Toggle a property panel from its trigger without coupling the trigger to
    /// egui's selection-menu popup state.
    pub fn toggle(ctx: &Context, id: Id, clicked: bool) -> bool {
        let state_id = id.with("open");
        let was_open = ctx.data(|data| data.get_temp::<bool>(state_id)).unwrap_or(false);
        let open = if clicked { !was_open } else { was_open };
        if clicked {
            ctx.data_mut(|data| data.insert_temp(state_id, open));
        }
        open
    }

    pub fn dismiss(ctx: &Context, id: Id) {
        ctx.data_mut(|data| data.insert_temp(id.with("open"), false));
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
            let (header, _) = ui.allocate_exact_size(vec2(width, header_height), Sense::hover());
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
        let bounds = ctx.content_rect();
        let stored = Self::position(ctx, self.id, self.initial_pos);
        let x = if stored.x.is_finite() { stored.x } else { bounds.left() + 8.0 };
        let y = if stored.y.is_finite() { stored.y } else { bounds.top() + 8.0 };
        let pos = pos2(
            x.clamp(bounds.left() + 8.0, (bounds.right() - width - 8.0).max(bounds.left() + 8.0)),
            y.clamp(bounds.top() + 8.0, (bounds.bottom() - height - 8.0).max(bounds.top() + 8.0)),
        );
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
            ctx.data_mut(|d| d.insert_temp(self.id.with("height"), result.response.rect.height()));
        }
        result
    }
}
