//! Icons from the design, as 24×24 SVG path data, painted with egui shapes.
//! Owner of the renderer: **T15a**.
//!
//! The renderer must support the path commands used below: `M m L l H h V v
//! C c S s A a Z z` (absolute + relative), multiple subpaths per string, and
//! tessellate curves/arcs to polylines (enough segments to look smooth at
//! 16–48 px). Strokes use round caps/joins.

use egui::{Color32, Painter, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    // ---- editor tools (order = toolbar order) ----
    Select,
    Crop,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Highlight,
    Blur,
    Pixelate,
    Step,
    Pen,
    Eyedropper,
    // ---- ui ----
    /// App glyph: four viewfinder corners.
    Frame,
    Undo,
    Redo,
    Copy,
    Save,
    OpenExternal,
    Folder,
    Search,
    Star,
    Trash,
    Calendar,
    Close,
    Check,
    Warning,
    Play,
    Gallery,
    Settings,
    Record,
    Volume,
    Mic,
    Keyboard,
    Monitor,
    Window,
    History,
    Scroll,
    Plus,
    Exit,
    Workflow,
}

impl Icon {
    /// Editor tools in toolbar order, matching `tool.*` shortcut ids.
    pub const TOOLS: [(Icon, &'static str); 13] = [
        (Icon::Select, "tool.select"),
        (Icon::Crop, "tool.crop"),
        (Icon::Arrow, "tool.arrow"),
        (Icon::Line, "tool.line"),
        (Icon::Rect, "tool.rect"),
        (Icon::Ellipse, "tool.ellipse"),
        (Icon::Text, "tool.text"),
        (Icon::Highlight, "tool.highlight"),
        (Icon::Blur, "tool.blur"),
        (Icon::Pixelate, "tool.pixelate"),
        (Icon::Step, "tool.step"),
        (Icon::Pen, "tool.pen"),
        (Icon::Eyedropper, "tool.eyedropper"),
    ];

    /// SVG path data in a 24×24 box (from the design canvas).
    pub fn path(self) -> &'static str {
        match self {
            Icon::Select => "M5 3l13 8-6 1.5-3 6.5z",
            Icon::Crop => "M6 2v14a2 2 0 0 0 2 2h14 M2 6h14a2 2 0 0 1 2 2v14",
            Icon::Arrow => "M5 19L19 5 M10 5h9v9",
            Icon::Line => "M5 19L19 5",
            Icon::Rect => "M4 6h16v12H4z",
            Icon::Ellipse => "M12 5c4.4 0 8 3.1 8 7s-3.6 7-8 7-8-3.1-8-7 3.6-7 8-7z",
            Icon::Text => "M5 7V4h14v3 M12 4v16 M9 20h6",
            Icon::Highlight => "M15 4l5 5-8 8H7v-5z M4 20h16",
            Icon::Blur => "M12 3s6 6.5 6 11a6 6 0 0 1-12 0c0-4.5 6-11 6-11z",
            Icon::Pixelate => "M4 4h6v6H4z M14 4h6v6h-6z M4 14h6v6H4z M14 14h6v6h-6z",
            Icon::Step => "M12 3a9 9 0 1 1 0 18a9 9 0 1 1 0-18z M10.5 9l2-1.5V16",
            Icon::Pen => "M4 20l4-1 11-11-3-3L5 16z M14 7l3 3",
            Icon::Eyedropper => "M15 3l6 6 M18 6l-9 9-4 1 1-4 9-9 M3 21l2-2",
            Icon::Frame => "M4 9V5a1 1 0 0 1 1-1h4 M15 4h4a1 1 0 0 1 1 1v4 M20 15v4a1 1 0 0 1-1 1h-4 M9 20H5a1 1 0 0 1-1-1v-4",
            Icon::Undo => "M9 14L4 9l5-5 M4 9h10a6 6 0 0 1 0 12h-3",
            Icon::Redo => "M15 14l5-5-5-5 M20 9H10a6 6 0 0 0 0 12h3",
            Icon::Copy => "M9 9h11v11H9z M5 15H4V4h11v1",
            Icon::Save => "M5 3h11l3 3v15H5z M8 3v5h7V3 M8 21v-7h8v7",
            Icon::OpenExternal => "M14 4h6v6 M20 4l-9 9 M18 14v6H4V6h6",
            Icon::Folder => "M3 6h6l2 2h10v11H3z",
            Icon::Search => "M11 4a7 7 0 1 1 0 14a7 7 0 1 1 0-14z M20 20l-4-4",
            Icon::Star => "M12 3l2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9z",
            Icon::Trash => "M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13",
            Icon::Calendar => "M4 6h16v14H4z M4 10h16 M8 3v4 M16 3v4",
            Icon::Close => "M6 6l12 12 M18 6L6 18",
            Icon::Check => "M5 12l5 5 9-10",
            Icon::Warning => "M12 3l9 16H3z M12 10v4 M12 17h.01",
            Icon::Play => "M8 5l11 7-11 7z",
            Icon::Gallery => "M4 4h7v7H4z M13 4h7v7h-7z M4 13h7v7H4z M13 13h7v7h-7z",
            Icon::Settings => "M12 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6z M12 2v3 M12 19v3 M2 12h3 M19 12h3 M5 5l2 2 M17 17l2 2 M5 19l2-2 M17 7l2-2",
            Icon::Record => "M12 8a4 4 0 1 1 0 8a4 4 0 1 1 0-8z M3 12a9 9 0 1 0 18 0a9 9 0 1 0-18 0",
            Icon::Volume => "M11 5L6 9H3v6h3l5 4z M16 9a4 4 0 0 1 0 6",
            Icon::Mic => "M9 3h6v10H9z M5 11a7 7 0 0 0 14 0 M12 18v3",
            Icon::Keyboard => "M3 7h18v10H3z M7 11h.01 M11 11h.01 M15 11h.01 M8 14h8",
            Icon::Monitor => "M3 5h18v12H3z M8 21h8 M12 17v4",
            Icon::Window => "M4 5h16v14H4z M4 9h16",
            Icon::History => "M4 12a8 8 0 1 0 3-6.2 M4 4v4h4",
            Icon::Scroll => "M7 3h10v18H7z M12 8v8 M9 13l3 3 3-3",
            Icon::Plus => "M12 5v14 M5 12h14",
            Icon::Exit => "M15 4h4v16h-4 M10 8l-4 4 4 4 M6 12h10",
            Icon::Workflow => "M4 7h16 M4 12h10 M4 17h6 M17 14l3 3-3 3",
        }
    }
}

/// Stroke the icon into `rect` (scaled from 24×24, centred, aspect kept).
/// `width` is the stroke width in 24-unit space (design: 1.75 for 20–24 px
/// icons, 2.0–2.6 for 12–16 px).
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    // TODO(T15a): parse `icon.path()` and stroke it. Placeholder so callers
    // can be built against the API meanwhile.
    let _ = (icon, width);
    painter.rect_stroke(rect.shrink(rect.width() * 0.2), 2.0, egui::Stroke::new(1.0_f32, color));
}

/// Fill the icon's closed subpaths (e.g. a filled star or play triangle).
pub fn paint_filled(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    // TODO(T15a)
    let _ = icon;
    painter.rect_filled(rect.shrink(rect.width() * 0.25), 2.0, color);
}
