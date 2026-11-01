//! UI3 SVG paths tessellated into egui meshes. No icon bitmaps or textures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Color32, Mesh, Painter, Rect, Shape, pos2, vec2};
use lyon_tessellation::path::{Path, math::point};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};
use usvg::tiny_skia_path::PathSegment;

#[path = "ui3_mapping.rs"]
mod mapping;

const VECTORS: &str = include_str!("../assets/ui3-vectors.json");
const PANEL_REFERENCE: &str = include_str!("../assets/panel-reference-icons.json");
const TAB_DOCUMENT_SVG: &str = include_str!("../assets/tab-document.svg");
// Home icon supplied by the owner for the desktop tab strip.
const HOME_TAB_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><path d="M9 21H7C4.79086 21 3 19.2091 3 17V10.7076C3 9.30887 3.73061 8.01175 4.92679 7.28679L9.92679 4.25649C11.2011 3.48421 12.7989 3.48421 14.0732 4.25649L19.0732 7.28679C20.2694 8.01175 21 9.30887 21 10.7076V17C21 19.2091 19.2091 21 17 21H15M9 21V17C9 15.3431 10.3431 14 12 14C13.6569 14 15 15.3431 15 17V21M9 21H15" stroke="#000" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
// Settings icon supplied by the owner for the desktop tab strip.
const SETTINGS_TAB_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><path d="m12.25,17.46c-.13-.01-.25-.05-.35-.11l-1.91-1.07-1.91,1.07c-.11.06-.22.09-.34.11h-.02c-.17,0-.25-.01-.34-.04-.97-.33-1.85-.82-2.62-1.48-.1-.08-.17-.18-.23-.3-.05-.11-.08-.24-.08-.36v-2.12s-1.9-1.07-1.9-1.07c-.11-.06-.21-.15-.28-.25-.07-.1-.12-.22-.15-.34-.2-.99-.2-1.99,0-2.98.02-.12.07-.24.15-.34.07-.1.17-.19.27-.25l1.88-1.07v-2.13c0-.13.04-.25.09-.36.05-.11.13-.21.22-.29.78-.66,1.66-1.16,2.63-1.48.09-.03.18-.05.28-.05.18.01.31.04.42.11l1.91,1.07,1.91-1.07c.11-.06.22-.09.34-.11h.03c.16,0,.24.01.33.04.97.33,1.85.82,2.63,1.48.1.08.17.18.23.3.05.12.08.24.08.37v2.12s1.89,1.08,1.89,1.08c.11.06.2.15.28.25.08.1.12.22.15.34.2.98.2,1.99,0,2.98-.02.12-.07.24-.15.34-.07.1-.16.18-.28.25l-1.89,1.07v2.13c0,.12-.04.25-.09.36-.06.12-.13.22-.23.3-.77.66-1.65,1.15-2.62,1.48-.08.03-.18.05-.28.05h-.07Zm-2.14-2.96c.17,0,.31.04.44.11l1.85,1.03c.53-.22.99-.48,1.4-.79v-2.06c0-.16.06-.32.14-.46.04-.07.08-.13.12-.2.08-.14.19-.26.33-.33l1.83-1.04c.07-.55.06-1.05,0-1.54l-1.83-1.04c-.14-.08-.25-.19-.33-.33-.04-.07-.08-.13-.12-.2-.09-.14-.13-.3-.13-.46v-2.05c-.47-.34-.93-.59-1.41-.79l-1.85,1.03c-.13.07-.28.11-.43.11h-.26c-.16,0-.31-.04-.44-.11l-1.85-1.03c-.54.22-.99.48-1.4.79v2.06c0,.16-.05.32-.14.46-.04.07-.08.13-.12.2-.08.14-.19.26-.33.33l-1.82,1.04c-.07.55-.07,1.05,0,1.54l1.83,1.04c.14.08.25.19.33.33.04.07.08.13.12.2.09.14.13.3.13.46v2.05c.48.34.93.59,1.41.79l1.85-1.03c.13-.07.28-.11.43-.11h.25Zm-.12-.88c-.24,0-.47-.02-.7-.07-.71-.14-1.35-.48-1.85-.99-.51-.51-.85-1.15-.99-1.85-.14-.7-.07-1.42.21-2.09.27-.66.73-1.23,1.33-1.62.6-.4,1.29-.61,2.01-.61.96,0,1.87.38,2.56,1.06.68.68,1.06,1.59,1.06,2.56,0,.72-.21,1.41-.61,2.01-.4.6-.96,1.06-1.62,1.33-.44.18-.91.28-1.38.28Zm0-5.49c-.12,0-.24.01-.37.04-.37.07-.7.25-.96.51-.26.26-.44.6-.51.96-.07.36-.04.74.11,1.09.14.34.38.63.69.84.31.21.67.32,1.04.32.5,0,.97-.19,1.33-.55.35-.36.55-.83.55-1.33,0-.37-.11-.73-.32-1.04-.21-.31-.5-.55-.84-.69-.23-.09-.47-.14-.72-.14Z" fill="#000"/></svg>"##;
const CHEVRON_SVG: &str = include_str!("../assets/chevron.svg");
const TYPOGRAPHY_SVGS: [&str; 9] = [
    include_str!("../assets/typ-line-height.svg"),
    include_str!("../assets/typ-letter-spacing.svg"),
    include_str!("../assets/typ-align-left.svg"),
    include_str!("../assets/typ-align-center.svg"),
    include_str!("../assets/typ-align-right.svg"),
    include_str!("../assets/typ-align-top.svg"),
    include_str!("../assets/typ-align-middle.svg"),
    include_str!("../assets/typ-align-bottom.svg"),
    include_str!("../assets/typ-settings.svg"),
];
const TYPOGRAPHY_DIALOG_SVGS: [&str; 13] = [
    include_str!("../assets/typ-dialog-justify.svg"),
    include_str!("../assets/typ-dialog-none.svg"),
    include_str!("../assets/typ-dialog-underline.svg"),
    include_str!("../assets/typ-dialog-strike.svg"),
    include_str!("../assets/typ-dialog-upper.svg"),
    include_str!("../assets/typ-dialog-lower.svg"),
    include_str!("../assets/typ-dialog-title.svg"),
    include_str!("../assets/typ-dialog-small-caps.svg"),
    include_str!("../assets/typ-dialog-trim-standard.svg"),
    include_str!("../assets/typ-dialog-trim-cap.svg"),
    include_str!("../assets/typ-dialog-list-bullets.svg"),
    include_str!("../assets/typ-dialog-list-numbered.svg"),
    include_str!("../assets/typ-dialog-truncate.svg"),
];
// The 12 × 12 unlink mark in the owner-supplied color-control design (node wHJkI).
const COLOR_DETACH_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12.0062275 12.0053263"><path d="M2.11161 5.65209c0.09375-0.09389 0.22096-0.14668 0.35364-0.14677 0.13268-0.00009 0.25997 0.05252 0.35386 0.14627 0.09389 0.09375 0.14668 0.22096 0.14678 0.35365 0.00009 0.13268-0.05252 0.25997-0.14628 0.35385l-1.232 1.232c-0.36432 0.3772-0.5659 0.88241-0.56135 1.40681 0.00456 0.52439 0.21489 1.02602 0.58571 1.39683 0.37082 0.37082 0.87244 0.58116 1.39684 0.58571 0.52439 0.00456 1.0296-0.19703 1.4068-0.56135l1.232-1.232c0.0943-0.09108 0.2206-0.14148 0.3517-0.14033 0.1311 0.00114 0.2565 0.05372 0.34921 0.14642 0.0927 0.0927 0.14529 0.21811 0.14643 0.34921 0.00114 0.1311-0.04926 0.2574-0.14034 0.3517l-1.232 1.232c-0.56581 0.54647-1.32361 0.84886-2.11021 0.84202-0.78659-0.00683-1.53903-0.32234-2.09525-0.87856-0.55622-0.55623-0.87173-1.30866-0.87856-2.09525-0.00684-0.78659 0.29555-1.5444 0.84202-2.11021l1.231-1.232z m-1.964-5.501c0.0813-0.08165 0.18831-0.13272 0.30292-0.14456 0.11461-0.01184 0.2298 0.01626 0.32608 0.07956l0.078 0.065 11 11 0.064 0.078c0.06373 0.09615 0.09225 0.21139 0.08073 0.32617-0.01152 0.11478-0.06238 0.22205-0.14395 0.30362-0.08157 0.08157-0.18884 0.13243-0.30361 0.14394-0.11478 0.01152-0.23001-0.017-0.32617-0.08073l-0.078-0.064-11-11-0.065-0.078c-0.06356-0.09637-0.09182-0.21177-0.07997-0.3266 0.01185-0.11484 0.06307-0.22103 0.14497-0.3024z m6.844 0.626c0.57061-0.51715 1.31824-0.79506 2.0881-0.77619 0.76986 0.01886 1.50298 0.33306 2.04759 0.87753 0.5446 0.54447 0.85897 1.27752 0.87801 2.04738 0.01905 0.76985-0.25869 1.51755-0.7757 2.08828l-0.107 0.111-1.232 1.233c-0.0943 0.09108-0.2206 0.14148-0.3517 0.14034-0.1311-0.00114-0.2565-0.05372-0.34921-0.14643-0.0927-0.0927-0.14529-0.21811-0.14643-0.34921-0.00114-0.1311 0.04926-0.2574 0.14034-0.3517l1.232-1.233 0.138-0.151c0.30985-0.38458 0.46661-0.87012 0.44014-1.36328-0.02646-0.49316-0.23428-0.95914-0.58349-1.30837-0.34922-0.34922-0.8152-0.55704-1.30836-0.58349-0.49316-0.02646-0.9787 0.1303-1.36329 0.44014l-0.15 0.138-1.233 1.232c-0.0943 0.09108-0.2206 0.14148-0.3517 0.14034-0.1311-0.00114-0.2565-0.05372-0.34921-0.14643-0.0927-0.0927-0.14529-0.21811-0.14643-0.34921-0.00114-0.1311 0.04926-0.2574 0.14034-0.3517l1.232-1.231 0.111-0.107z" fill="white"/></svg>"#;
// Variable affordances from the owner's number-field reference (node BavO5).
const NUMBER_VARIABLE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 12"><path fill="white" fill-rule="evenodd" d="M2 3.693l4-2.31 4 2.31v4.619l-4 2.31-4-2.31V3.693zm4-3.463l5 2.887v5.773l-5 2.887-5-2.887V3.116L6 .23zm1 5.77a1 1 0 1 1-2 0 1 1 0 0 1 2 0z"/></svg>"#;
const NUMBER_DETACH_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10.031431 10.033246"><path fill="white" d="M1.46521 5.02922c.09389-.09375.22117-.14637.35385-.14628.13268.00009.25989.05289.35365.14678.09375.09389.14637.22117.14628.35385-.00009.13268-.05289.25989-.14678.35365l-.732.732c-.14327.13837-.25754.30389-.33615.48689-.07861.18301-.11999.37984-.12172.57901-.00173.19917.03622.39669.11164.58103.07542.18434.1868.35182.32764.49266.14084.14084.30832.25222.49266.32764.18434.07542.38186.11337.58103.11165.19917-.00173.396-.04311.57901-.12173.18301-.07861.34852-.19289.48689-.33615l.732-.732c.0943-.09108.2206-.14148.3517-.14034.1311.00114.2565.05372.34921.14643.0927.0927.14529.21811.14643.34921.00114.1311-.04926.2574-.14034.3517l-.732.732c-.47151.45539-1.10301.70738-1.7585.70169-.65549-.0057-1.28252-.26862-1.74605-.73214-.46352-.46352-.72644-1.09055-.73213-1.74605-.0057-.65549.24629-1.287.70168-1.7585l.732-.733zM.14721.17722C.22863.09572.3357.04484.45031.03318.56492.02152.68004.04979.77621.11322l.078.064 9 9 .064.078c.06432.09614.09331.21159.08205.32671-.01126.11512-.06209.22277-.14382.30461-.08173.08185-.18931.13283-.3044.14426-.1151.01143-.2306-.0174-.32683-.08158l-.078-.065-9-9-.065-.078C.01991.72994-.0082.61475.00365.50014.01549.38553.06656.27852.14821.17622zm5.586.586c.23062-.23878.50648-.42923.81149-.56025C6.84971.07195 7.17776.00298 7.50971.00009c.33195-.00288.66115.06037.96839.18608.30724.1257.58637.31134.8211.54606.23473.23473.42037.51386.54607.82111.1257.30724.18896.63644.18607.96838-.00289.33195-.07185.66-.20288.96501-.13102.30501-.32148.58087-.56025.81149l-.732.733c-.0943.09108-.2206.14148-.3517.14034-.1311-.00114-.2565-.05372-.34921-.14643-.0927-.0927-.14529-.21811-.14643-.34921-.00114-.1311.04926-.2574.14034-.3517l.732-.733c.28139-.28126.43953-.66279.43963-1.06065.00009-.39786-.15786-.77946-.43913-1.06085-.28126-.28139-.66279-.43953-1.06065-.43963-.39786-.00009-.77946.15786-1.06085.43913l-.733.732c-.0943.09108-.2206.14148-.3517.14034-.1311-.00114-.2565-.05372-.34921-.14643-.0927-.0927-.14529-.21811-.14643-.34921-.00114-.1311.04926-.2574.14034-.3517l.733-.731z"/></svg>"#;
// Exact 24 × 24 icon compositions from the Lizard toolbar design.
const TOOLBAR_SVGS: [&str; 12] = [
    include_str!("../assets/toolbar-move.svg"),
    include_str!("../assets/toolbar-frame.svg"),
    include_str!("../assets/toolbar-rectangle.svg"),
    include_str!("../assets/toolbar-pen.svg"),
    include_str!("../assets/toolbar-text.svg"),
    include_str!("../assets/toolbar-comment.svg"),
    include_str!("../assets/toolbar-actions.svg"),
    "", // Chevron is painted from the shared SVG.
    include_str!("../assets/toolbar-mode-draw.svg"),
    include_str!("../assets/toolbar-mode-design.svg"),
    include_str!("../assets/toolbar-mode-prototype.svg"),
    include_str!("../assets/toolbar-mode-dev.svg"),
];

// The two spacing glyphs drawn in the owner's New Trasso Icons document.
const SELECTION_SPACING_SVGS: [&str; 2] =
    [include_str!("../assets/selection-spacing-horizontal.svg"), include_str!("../assets/selection-spacing-vertical.svg")];

// Exact paths from the owner's image toolbar reference (node pxhiB).
const IMAGE_ACTION_SVGS: [&str; 7] = [
    include_str!("../assets/image-crop.svg"),
    include_str!("../assets/image-select-area.svg"),
    include_str!("../assets/image-remove-background.svg"),
    include_str!("../assets/image-edit-prompt.svg"),
    include_str!("../assets/image-expand.svg"),
    include_str!("../assets/image-enhance.svg"),
    include_str!("../assets/image-vectorize.svg"),
];

pub(super) fn paint_toolbar(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    if index == 7 {
        paint_chevron(painter, rect.center(), 5.24, ChevronDirection::Down, color);
        return;
    }
    static ICONS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = ICONS.get_or_init(|| TOOLBAR_SVGS.iter().map(|svg| build(svg, true)).collect());
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
    }
}

pub(super) fn paint_color_detach(painter: &Painter, rect: Rect, color: Color32) {
    static GEOMETRY: OnceLock<Option<Geometry>> = OnceLock::new();
    if let Some(geometry) = GEOMETRY.get_or_init(|| build(COLOR_DETACH_SVG, true)) {
        paint_geometry(painter, rect, geometry, color, [12.0062275, 12.005_326]);
    }
}

pub(super) fn paint_number_variable(painter: &Painter, rect: Rect, color: Color32) {
    static GEOMETRY: OnceLock<Option<Geometry>> = OnceLock::new();
    if let Some(geometry) = GEOMETRY.get_or_init(|| build(NUMBER_VARIABLE_SVG, true)) {
        paint_geometry(painter, rect, geometry, color, [12.0, 12.0]);
    }
}

pub(super) fn paint_number_detach(painter: &Painter, rect: Rect, color: Color32) {
    static GEOMETRY: OnceLock<Option<Geometry>> = OnceLock::new();
    if let Some(geometry) = GEOMETRY.get_or_init(|| build(NUMBER_DETACH_SVG, true)) {
        paint_geometry(painter, rect, geometry, color, [10.031431, 10.033246]);
    }
}
const LAYER_SVGS: [(&str, [f32; 2]); 22] = [
    (include_str!("../assets/layer-frame.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-layout-horizontal.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-layout-vertical.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-layout-grid.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-text.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-absolute.svg"), [16.0, 16.0]),
    ("", [5.237, 3.122]),
    ("", [3.122, 5.237]),
    (include_str!("../assets/layer-mask.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-ellipse.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-ellipse-mask.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-component.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-instance.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-vector.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-image.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-star.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-polygon.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-arrow.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-line.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-rectangle.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-group.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-collapse.svg"), [24.0, 24.0]),
];
const NAV_SVGS: [(&str, [f32; 2]); 5] = [
    (include_str!("../assets/nav-logo.svg"), [85.84124, 94.0]),
    (include_str!("../assets/nav-file.svg"), [10.0, 14.0]),
    (include_str!("../assets/nav-agent.svg"), [14.0, 14.0]),
    (include_str!("../assets/nav-assets.svg"), [14.0, 14.0]),
    (include_str!("../assets/nav-variables.svg"), [12.0, 13.23847]),
];
const HEADER_SVGS: [(&str, [f32; 2]); 14] = [
    (include_str!("../assets/header-collapse-panel.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-present.svg"), [24.0, 24.0]),
    ("", [16.0, 16.0]),
    (include_str!("../assets/header-avatar-ring.svg"), [30.0, 40.0]),
    (include_str!("../assets/header-create-component.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-mask.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-mask-tertiary.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-union.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-union-tertiary.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-edit-object.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-select-matching.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-text-link.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-text-variable.svg"), [24.0, 24.0]),
    (include_str!("../assets/agent-more.svg"), [15.0, 3.0]),
];
const POSITION_SVGS: [&str; 10] = [
    include_str!("../assets/position-align-left.svg"),
    include_str!("../assets/position-align-hcenter.svg"),
    include_str!("../assets/position-align-right.svg"),
    include_str!("../assets/position-align-top.svg"),
    include_str!("../assets/position-align-vcenter.svg"),
    include_str!("../assets/position-align-bottom.svg"),
    include_str!("../assets/position-rotation.svg"),
    include_str!("../assets/position-rotate-right.svg"),
    include_str!("../assets/position-flip-horizontal.svg"),
    include_str!("../assets/position-flip-vertical.svg"),
];
const VECTOR_POINT_SVGS: [&str; 4] = [
    include_str!("../assets/vector-point-mirror-none.svg"),
    include_str!("../assets/vector-point-mirror-angle.svg"),
    include_str!("../assets/vector-point-mirror-angle-length.svg"),
    include_str!("../assets/vector-point-more.svg"),
];
const APPEARANCE_SVGS: [&str; 9] = [
    include_str!("../assets/appearance-visible.svg"),
    include_str!("../assets/appearance-blend.svg"),
    include_str!("../assets/appearance-opacity.svg"),
    include_str!("../assets/appearance-corners.svg"),
    include_str!("../assets/appearance-corner-top-left.svg"),
    include_str!("../assets/appearance-corner-top-right.svg"),
    include_str!("../assets/appearance-corner-bottom-left.svg"),
    include_str!("../assets/appearance-corner-bottom-right.svg"),
    include_str!("../assets/appearance-corner-settings.svg"),
];
const COLORPICKER_SVGS: [(&str, [f32; 2]); 17] = [
    (include_str!("../assets/colorpicker-solid.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-gradient.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-pattern.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-image.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-video.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-shader.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-contrast.svg"), [14.0, 14.0]),
    ("", [5.237, 3.122]),
    (include_str!("../assets/colorpicker-add-style.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-close.svg"), [10.009888, 10.005533]),
    (include_str!("../assets/colorpicker-eyedropper.svg"), [13.980761, 13.977587]),
    (include_str!("../assets/colorpicker-gradient-flip.svg"), [12.000786, 13.000001]),
    (include_str!("../assets/colorpicker-gradient-orientation.svg"), [10.001458, 14.258431]),
    (include_str!("../assets/colorpicker-gradient-remove.svg"), [12.0, 1.0]),
    (include_str!("../assets/colorpicker-pattern-source.svg"), [13.0, 13.0]),
    (include_str!("../assets/colorpicker-pattern-scale.svg"), [12.0, 12.0]),
    (include_str!("../assets/colorpicker-image-create.svg"), [13.939, 13.939]),
];
const PROPERTY_SVGS: [&str; 4] = [
    include_str!("../assets/property-styles.svg"),
    include_str!("../assets/property-stroke-weight.svg"),
    include_str!("../assets/property-stroke-settings.svg"),
    include_str!("../assets/property-stroke-sides.svg"),
];
const STROKE_SIDE_SVGS: [(&str, [f32; 2]); 5] = [
    (include_str!("../assets/stroke-side-check.svg"), [8.992507, 8.487045]),
    (include_str!("../assets/stroke-side-top.svg"), [12.0, 11.0]),
    (include_str!("../assets/stroke-side-bottom.svg"), [12.0, 11.0]),
    (include_str!("../assets/stroke-side-left.svg"), [11.0, 12.0]),
    (include_str!("../assets/stroke-side-right.svg"), [11.0, 12.0]),
];
const LAYOUT_SVGS: [(&str, [f32; 2]); 22] = [
    (include_str!("../assets/layout-aspect-ratio.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-auto.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-free.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-vertical.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-horizontal.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-grid.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-wrap.svg"), [16.0, 16.0]),
    (include_str!("../assets/layout-gap-vertical.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-settings.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-horizontal.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-vertical.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-individual.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-auto-active.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-individual-active.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-left.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-right.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-top.svg"), [24.0, 24.0]),
    (include_str!("../assets/layout-padding-bottom.svg"), [24.0, 24.0]),
    (include_str!("../assets/text-layout-auto-width.svg"), [24.0, 24.0]),
    (include_str!("../assets/text-layout-auto-height.svg"), [24.0, 24.0]),
    (include_str!("../assets/text-layout-fixed.svg"), [24.0, 24.0]),
    (include_str!("../assets/text-layout-aspect-ratio.svg"), [24.0, 24.0]),
];
const SIZING_SVGS: [(&str, [f32; 2]); 12] = [
    (include_str!("../assets/sizing-fixed.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-hug.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-fill.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-check.svg"), [18.0, 24.0]),
    (include_str!("../assets/sizing-min.svg"), [16.0, 16.0]),
    (include_str!("../assets/sizing-max.svg"), [16.0, 16.0]),
    (include_str!("../assets/sizing-variable.svg"), [12.0, 12.0]),
    (include_str!("../assets/sizing-height-fixed.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-height-hug.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-height-fill.svg"), [24.0, 24.0]),
    (include_str!("../assets/sizing-height-min.svg"), [16.0, 16.0]),
    (include_str!("../assets/sizing-height-max.svg"), [16.0, 16.0]),
];
const AGENT_SVGS: [(&str, [f32; 2]); 17] = [
    ("", [5.001787, 9.001525]),
    (include_str!("../assets/agent-more.svg"), [15.0, 3.0]),
    (include_str!("../assets/agent-emblem.svg"), [16.499786, 16.499025]),
    (include_str!("../assets/agent-polish.svg"), [13.498501, 12.998539]),
    (include_str!("../assets/nav-agent.svg"), [14.0, 14.0]),
    (include_str!("../assets/agent-plugins.svg"), [14.0, 13.0]),
    ("", [9.001525, 5.001787]),
    (include_str!("../assets/colorpicker-add-style.svg"), [12.0, 12.0]),
    (include_str!("../assets/agent-library.svg"), [14.0, 11.74961]),
    (include_str!("../assets/agent-mic.svg"), [10.0, 14.0]),
    (include_str!("../assets/agent-send.svg"), [10.006924, 13.000607]),
    (
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 13.99993896484375 14" preserveAspectRatio="xMidYMid meet"><path d="M9.21143 0.58789q-0.33496 0.01367-0.69385 0.11963-0.35547 0.10596-0.64942 0.27344-0.2085 0.12646-0.35888 0.25293-0.14697 0.12305-0.38282 0.3623-0.78613 0.78271-2.87109 2.92578-2.08496 2.14307-2.17041 2.2251-0.38965 0.47852-0.62891 1.09375-0.23926 0.61523-0.28027 1.24414-0.05469 0.88184 0.25293 1.69531 0.30762 0.81006 0.93652 1.44239 0.95361 0.9502 2.33789 1.16211 0.21191 0.02734 0.54688 0.02734 0.33496 0 0.54687-0.02734 1.14844-0.18115 1.99952-0.86817 0.1709-0.12647 0.58789-0.54687 0.42041-0.42041 2.18408-2.2251l2.34131-2.40967q0.24951-0.24951 0.26318-0.30762l0-0.01367q0.0581-0.16748 0.02735-0.32812-0.02734-0.16064-0.15381-0.28711-0.16748-0.16748-0.41699-0.16748l-0.04444 0q-0.08203-0.01367-0.14014 0.01367-0.0957 0.05469-0.28711 0.23242-0.18799 0.17432-0.62206 0.62207l-1.72266 1.75q-2.46436 2.51904-2.61817 2.64551-0.58789 0.54688-1.45605 0.71435-0.16748 0.04102-0.50928 0.03418-0.3418-0.00684-0.52636-0.04785-0.65625-0.14014-1.16211-0.50586-0.2085-0.14014-0.44092-0.38965-0.229-0.25293-0.35547-0.47851-0.46143-0.78271-0.39307-1.7124 0.07178-0.93311 0.65967-1.63379 0.10938-0.14014 2.15332-2.2251l2.50879-2.54639q0.51611-0.5332 0.61523-0.60498 0.2666-0.19482 0.61524-0.29394 0.16748-0.04102 0.41357-0.04785 0.24609-0.00684 0.41358 0.04785 0.43408 0.09912 0.7622 0.39306 0.32813 0.29395 0.48194 0.68702 0.05811 0.18115 0.07861 0.2871 0.02051 0.10254 0.02051 0.32813 0 0.22559-0.02051 0.33154-0.02051 0.10254-0.07861 0.28369-0.08203 0.23926-0.23584 0.43409-0.05811 0.07178-2.59082 2.65918l-1.77735 1.8081q-0.44775 0.46143-0.64599 0.64258-0.19482 0.18115-0.28028 0.22559-0.05469 0.02734-0.14013 0.02734l-0.05469 0q-0.08545 0-0.1333-0.00684-0.04785-0.00684-0.11963-0.04785-0.18115-0.09912-0.28027-0.28027-0.04102-0.08545-0.04102-0.23926 0-0.09912 0.01367-0.14697 0.01367-0.04785 0.04102-0.10938 0.03076-0.06494 0.47168-0.51953 0.44092-0.45459 1.8833-1.92431l2.51904-2.57715q0.19482-0.21191 0.24951-0.30762 0.05811-0.09912 0.05811-0.25293 0-0.23926-0.16748-0.40674-0.09912-0.0957-0.18457-0.12988-0.08203-0.0376-0.22217-0.0376l-0.05469 0q-0.08545 0-0.14013 0.02734-0.08545 0.04443-0.27344 0.21875-0.18799 0.17432-0.62207 0.6084l-1.75 1.77735q-2.49512 2.5498-2.57715 2.6455-0.25293 0.28027-0.36572 0.71436-0.04102 0.14014-0.04102 0.40674 0 0.18115 0.00684 0.25976 0.00684 0.0752 0.03418 0.18799 0.11279 0.42041 0.38623 0.72803 0.27344 0.30762 0.68017 0.46143 0.28027 0.11279 0.61524 0.11279 0.33496 0 0.60156-0.0957 0.2666-0.09912 0.4751-0.25293 0.07178-0.05811 2.51904-2.54981 2.45068-2.4917 2.6626-2.73096 0.29395-0.32129 0.48193-0.72461 0.18799-0.40674 0.2461-0.85449 0.08203-0.646-0.13672-1.28857-0.21533-0.646-0.67676-1.1211-0.43408-0.43408-1.00146-0.66308-0.56738-0.23242-1.19629-0.20508z" fill="#A5A5A8"/></svg>"##,
        [13.999939, 14.0],
    ),
    (
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 13.99993896484375 14" preserveAspectRatio="xMidYMid meet"><path d="M7.7417 0.60156q-0.16748 0.02734-0.33496 0.1128-0.09912 0.06836-2.81641 2.87109l-2.88135 2.98047q-0.35205 0.36572-0.39306 0.44775l-0.01367 0.01367q-0.09912 0.21191-0.12647 0.44776-0.04102 0.37939 0.14014 0.68701 0.18115 0.30762 0.50244 0.46143l0.04443 0.02734q0.0957 0.04443 0.19483 0.0581 0.12646 0.02734 0.51953 0.02735l3.83496 0.02734-0.57422 1.79102q-0.57422 1.79443-0.57422 1.89013-0.04102 0.25293 0.09912 0.50586 0.14014 0.25293 0.37598 0.37598 0.39307 0.18457 0.77246 0 0.12646-0.06836 0.32129-0.24951l5.59863-5.7832q0.22559-0.22559 0.30762-0.42041 0.08545-0.19482 0.08545-0.4751 0-0.32129-0.16748-0.59473-0.16748-0.27344-0.46143-0.42725l-0.04443-0.01367q-0.0957-0.05811-0.19483-0.07177-0.14014-0.02734-0.51953-0.02735l-3.84863-0.02734 0.57422-1.79102q0.57422-1.79443 0.57422-1.89013 0.04102-0.25293-0.09912-0.50586-0.14014-0.25293-0.39307-0.37598-0.24951-0.12646-0.50244-0.07178z m-0.92285 3.19239q-0.37939 1.20313-0.39307 1.32959-0.02734 0.19482 0.02051 0.3999 0.05127 0.20166 0.16064 0.36914 0.07178 0.09912 0.20508 0.21875 0.1333 0.11963 0.24268 0.17431l0.04443 0.02735q0.10938 0.05811 0.2085 0.07178 0.14014 0.01367 0.51953 0.02734l3.82129 0.01367-2.42334 2.50537q-2.41992 2.4917-2.42676 2.4917-0.00684 0 0.37598-1.20996 0.38623-1.20996 0.3999-1.33643 0.02734-0.19482-0.02393-0.39648-0.04785-0.20508-0.15722-0.35889-0.05811-0.08545-0.16407-0.19482-0.10254-0.11279-0.18798-0.15723-0.16748-0.10938-0.37598-0.16748-0.07178-0.01367-2.19775-0.02734l-2.11573 0 2.42334-2.50537q2.41992-2.4917 2.42676-2.4917 0.00684 0-0.38281 1.2168z" fill="#FFBF00"/></svg>"##,
        [13.999939, 14.0],
    ),
    (
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 13.99993896484375 14" preserveAspectRatio="xMidYMid meet"><path d="M2.03027 1.20313q-0.22559 0.07178-0.42724 0.23242-0.20166 0.16064-0.30078 0.36914l-0.02735 0.03076q-0.05811 0.12305-0.07177 0.2085-0.02734 0.12646-0.02735 0.44775l0 1.04932 0 1.66796q0 0.24951 0.02735 0.34864 0.07178 0.28027 0.32128 0.51953 0.11279 0.10938 0.22559 0.18115 0.11279 0.06836 0.2666 0.11279l0.12647 0.04102 3.06591 0q0.24951-0.01367 0.33497-0.02734 0.30762-0.08545 0.52978-0.30762 0.22559-0.22559 0.31104-0.5332 0.01367-0.08545 0.02734-0.33497l0-3.06591-0.04102-0.12647q-0.04443-0.16748-0.11621-0.27344-0.06836-0.10596-0.1914-0.23242-0.15381-0.15381-0.29395-0.22217l-0.01709-0.01367q-0.12305-0.05811-0.22217-0.07178-0.12646-0.02734-0.47509-0.02734l-1.14844 0-1.52783 0q-0.24951 0.01367-0.34864 0.02734z m6.43946-0.00001q-0.28027 0.05811-0.53321 0.30762-0.12305 0.12646-0.19482 0.23242-0.06836 0.10596-0.11279 0.27344l-0.04102 0.12647 0 3.06591q0.01367 0.2666 0.02734 0.33497 0.08545 0.30762 0.30762 0.5332 0.22559 0.22217 0.5332 0.30762 0.06836 0.01367 0.33497 0.02734l3.06591 0 0.12647-0.04102q0.16748-0.04443 0.27344-0.11279 0.10596-0.07178 0.23242-0.19482 0.23584-0.23926 0.30761-0.53321 0.02734-0.12646 0.03418-1.74316 0.00684-1.6167-0.03418-1.72949-0.07178-0.30762-0.30419-0.53662-0.229-0.23242-0.53663-0.31788-0.09912-0.02734-0.33496-0.02734l-1.41504 0q-1.62354 0-1.73632 0.02734z m-3.21973 2.59083l0 1.45605-2.92578 0 0-2.92578 2.92578 0 0 1.46973z m6.42578 0l0 1.45605-2.92578 0 0-2.92578 2.92578 0 0 1.46973z m-9.47803 3.79394q-0.39307 0.04102-0.68701 0.34863-0.23584 0.22559-0.30761 0.50586-0.02734 0.09912-0.02735 0.34864l0 1.41503 0 1.30225q0 0.32129 0.02735 0.44775 0.01367 0.08545 0.07177 0.2085l0.02735 0.03076q0.09912 0.22217 0.30761 0.38281 0.2085 0.16065 0.44776 0.21875 0.11279 0.04102 1.72949 0.03418 1.6167-0.00684 1.74316-0.03418 0.29395-0.07178 0.53321-0.30761 0.12305-0.12646 0.1914-0.23242 0.07178-0.10596 0.11621-0.27344l0.04102-0.12647 0-3.06591q-0.01367-0.24951-0.02734-0.33497-0.08545-0.30762-0.31788-0.52978-0.229-0.22559-0.52294-0.31104-0.06836-0.01367-0.34864-0.02734l-1.31592 0q-1.55518-0.01367-1.68164 0z m6.42579 0q-0.22217 0.02734-0.37598 0.11279-0.23926 0.11279-0.3999 0.30762-0.16064 0.19482-0.23243 0.44775-0.01367 0.06836-0.02734 0.33497l0 3.06591 0.04102 0.12647q0.04443 0.16748 0.11279 0.27344 0.07178 0.10596 0.19482 0.23242 0.23926 0.23584 0.53321 0.30761 0.12646 0.02734 1.74316 0.03418 1.6167 0.00684 1.72949-0.03418 0.23926-0.0581 0.44776-0.21875 0.2085-0.16065 0.30761-0.38281l0.02735-0.03076q0.05811-0.12305 0.07178-0.2085 0.02734-0.12646 0.02734-0.44775l0-2.59082q0-0.33496-0.02734-0.46143-0.01367-0.08545-0.07178-0.20849l-0.01367-0.03076q-0.06836-0.12646-0.22217-0.28028-0.15381-0.15381-0.29395-0.22216l-0.03076-0.01368q-0.12305-0.07178-0.2085-0.08545-0.11279-0.01367-0.4204-0.02734l-1.23047 0q-1.56885 0-1.68164 0.01367l0-0.01367z m-3.37354 2.61816l0 1.46973-2.92578 0 0-2.92578 2.92578 0 0 1.45605z m6.42578 0l0 1.46973-2.92578 0 0-2.92578 2.92578 0 0 1.45605z" fill="#999999"/></svg>"##,
        [13.999939, 14.0],
    ),
    ("", [13.999939, 14.0]),
    ("", [13.999939, 14.0]),
    (
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 13.99993896484375 14" preserveAspectRatio="xMidYMid meet"><path d="M6.84619 2.35156q-0.08545 0.01367-0.15723 0.06494-0.06836 0.04785-2.16699 2.14649l-1.66455 1.69531q-0.36572 0.3623-0.46484 0.50244-0.05469 0.08545-0.05469 0.19824l0 0.04102q0 0.23926 0.18115 0.42041 0.11279 0.11279 0.28028 0.14697 0.16748 0.03418 0.32129-0.02051 0.08545-0.02734 0.32128-0.25976 0.23926-0.23242 1.37403-1.36377 1.58252-1.58252 1.59619-1.58252 0.01367 0 0.01367 3.42822l0 3.01123q0 0.44775 0.03418 0.53321 0.0376 0.08203 0.11963 0.16748 0.08545 0.08203 0.18799 0.1333 0.10596 0.04785 0.23242 0.04785 0.12646 0 0.229-0.04785 0.10596-0.05127 0.18799-0.1333 0.08545-0.08545 0.11963-0.16748 0.0376-0.08545 0.0376-0.53321l0-3.01123q0-3.42822 0.01367-3.42822 0.01367 0 1.59619 1.58252 1.13477 1.13135 1.37061 1.36377 0.23926 0.23242 0.32471 0.25976 0.15381 0.05469 0.32128 0.02051 0.16748-0.03418 0.28028-0.14697 0.18115-0.18115 0.18115-0.42041l0-0.04102q0-0.11279-0.05469-0.19824-0.09912-0.14014-0.46484-0.51611l-1.67822-1.68164q-2.10205-2.1123-2.17041-2.14649-0.06836-0.0376-0.16065-0.06494-0.09229-0.02734-0.14014-0.02734-0.04785 0-0.14697 0.02734z" fill="#202124"/></svg>"##,
        [13.999939, 14.0],
    ),
];
const ALIGNMENT_SVGS: [&str; 6] = [
    include_str!("../assets/alignment-column-left.svg"),
    include_str!("../assets/alignment-column-center.svg"),
    include_str!("../assets/alignment-column-right.svg"),
    include_str!("../assets/alignment-row-top.svg"),
    include_str!("../assets/alignment-row-middle.svg"),
    include_str!("../assets/alignment-row-bottom.svg"),
];
/// Extra width in the source SVG's 24-unit view box. A 20-point toolbar icon
/// gains one third of a point, keeping small icons legible without soft edges.
const STROKE_BOOST: f32 = 0.4;

#[derive(Clone)]
struct Part {
    positions: Vec<[f32; 2]>,
    indices: Vec<u32>,
    opacity: f32,
}

#[derive(Clone)]
struct Geometry(Vec<Part>);

#[derive(Clone, Default)]
struct TabDocumentTextures(Arc<Mutex<HashMap<u32, egui::TextureHandle>>>);

type RasterKey = (usize, usize, u16, u16, bool);

#[derive(Clone, Default)]
struct IconTextures(Arc<Mutex<HashMap<RasterKey, egui::TextureHandle>>>);

fn sources() -> &'static [String] {
    static SOURCES: OnceLock<Vec<String>> = OnceLock::new();
    SOURCES.get_or_init(|| serde_json::from_str(VECTORS).unwrap_or_default())
}

fn cached(index: usize, exact: bool) -> Option<Arc<Geometry>> {
    type GeometryCache = Mutex<HashMap<(usize, bool), Option<Arc<Geometry>>>>;
    static CACHE: OnceLock<GeometryCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(value) = cache.lock().ok()?.get(&(index, exact)) {
        return value.clone();
    }
    let geometry = sources().get(index).and_then(|svg| build(svg, exact)).map(Arc::new);
    cache.lock().ok()?.insert((index, exact), geometry.clone());
    geometry
}

fn to_path(source: &usvg::tiny_skia_path::Path) -> Path {
    let mut builder = Path::builder();
    let mut open = false;
    for segment in source.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                if open {
                    builder.end(false);
                }
                builder.begin(point(p.x, p.y));
                open = true;
            }
            PathSegment::LineTo(p) if open => {
                builder.line_to(point(p.x, p.y));
            }
            PathSegment::QuadTo(c, p) if open => {
                builder.quadratic_bezier_to(point(c.x, c.y), point(p.x, p.y));
            }
            PathSegment::CubicTo(c1, c2, p) if open => {
                builder.cubic_bezier_to(point(c1.x, c1.y), point(c2.x, c2.y), point(p.x, p.y));
            }
            PathSegment::Close if open => {
                builder.end(true);
                open = false;
            }
            _ => {}
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

fn part(buffer: VertexBuffers<lyon_tessellation::math::Point, u32>, opacity: f32) -> Option<Part> {
    if buffer.indices.is_empty() {
        return None;
    }
    Some(Part { positions: buffer.vertices.into_iter().map(|p| [p.x, p.y]).collect(), indices: buffer.indices, opacity })
}

fn add_path(geometry: &mut Geometry, source: &usvg::Path, exact: bool) {
    let transformed = source.data().clone().transform(source.abs_transform());
    let Some(transformed) = transformed else { return };
    let path = to_path(&transformed);
    let width =
        source.stroke().map_or(if exact { 0.0 } else { STROKE_BOOST }, |stroke| stroke.width().get() + if exact { 0.0 } else { STROKE_BOOST });
    if !exact {
        let mut fringe = VertexBuffers::new();
        let fringe_options = StrokeOptions::default().with_line_width(width + 0.55).with_tolerance(0.05);
        if StrokeTessellator::new()
            .tessellate_path(&path, &fringe_options, &mut BuffersBuilder::new(&mut fringe, |vertex: StrokeVertex<'_, '_>| vertex.position()))
            .is_ok()
            && let Some(part) = part(fringe, 0.22)
        {
            geometry.0.push(part);
        }
    }
    if let Some(fill) = source.fill() {
        let rule = match fill.rule() {
            usvg::FillRule::EvenOdd => FillRule::EvenOdd,
            usvg::FillRule::NonZero => FillRule::NonZero,
        };
        let options = FillOptions::default().with_fill_rule(rule).with_tolerance(0.05);
        let mut buffer = VertexBuffers::new();
        if FillTessellator::new()
            .tessellate_path(&path, &options, &mut BuffersBuilder::new(&mut buffer, |vertex: FillVertex<'_>| vertex.position()))
            .is_ok()
            && let Some(part) = part(buffer, 1.0)
        {
            geometry.0.push(part);
        }
    }
    // The published set consists of filled paths except for one stroked icon.
    // A narrow geometric stroke around the fills improves the toolbar weight.
    if width > 0.0 {
        let options = StrokeOptions::default().with_line_width(width).with_tolerance(0.05);
        let mut buffer = VertexBuffers::new();
        if StrokeTessellator::new()
            .tessellate_path(&path, &options, &mut BuffersBuilder::new(&mut buffer, |vertex: StrokeVertex<'_, '_>| vertex.position()))
            .is_ok()
            && let Some(part) = part(buffer, 1.0)
        {
            geometry.0.push(part);
        }
    }
}

fn build(svg: &str, exact: bool) -> Option<Geometry> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut geometry = Geometry(Vec::new());
    fn visit(nodes: &[usvg::Node], geometry: &mut Geometry, exact: bool) {
        for node in nodes {
            match node {
                usvg::Node::Path(path) => add_path(geometry, path, exact),
                usvg::Node::Group(group) => visit(group.children(), geometry, exact),
                _ => {}
            }
        }
    }
    visit(tree.root().children(), &mut geometry, exact);
    (!geometry.0.is_empty()).then_some(geometry)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChevronDirection {
    Down,
    Right,
    Up,
    Left,
}

/// Every disclosure chevron is the same SVG, rotated and sized at the call site.
pub(super) fn paint_chevron(painter: &Painter, center: egui::Pos2, painted_size: f32, direction: ChevronDirection, color: Color32) {
    static CHEVRONS: OnceLock<[Option<Geometry>; 4]> = OnceLock::new();
    let geometries = CHEVRONS.get_or_init(|| {
        let base = build(CHEVRON_SVG, true);
        std::array::from_fn(|index| {
            base.as_ref().map(|source| {
                let mut geometry = source.clone();
                for part in &mut geometry.0 {
                    for point in &mut part.positions {
                        let [x, y] = *point;
                        *point = match index {
                            1 => [y, 5.237 - x],
                            2 => [5.237 - x, 3.122 - y],
                            3 => [3.122 - y, x],
                            _ => [x, y],
                        };
                    }
                }
                geometry
            })
        })
    });
    let index = match direction {
        ChevronDirection::Down => 0,
        ChevronDirection::Right => 1,
        ChevronDirection::Up => 2,
        ChevronDirection::Left => 3,
    };
    if let Some(geometry) = &geometries[index] {
        let box_size = painted_size / 0.78;
        let view_box = if matches!(direction, ChevronDirection::Right | ChevronDirection::Left) { [3.122, 5.237] } else { [5.237, 3.122] };
        paint_geometry_optical(painter, Rect::from_center_size(center, vec2(box_size, box_size)), geometry, color, view_box);
    }
}

pub(super) fn paint(painter: &Painter, rect: Rect, icon: super::Icon, color: Color32) -> bool {
    let spacing = match icon {
        super::Icon::SelectionSpacingH => Some(0),
        super::Icon::SelectionSpacingV => Some(1),
        _ => None,
    };
    if let Some(index) = spacing {
        static ICONS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
        let icons = ICONS.get_or_init(|| SELECTION_SPACING_SVGS.iter().map(|svg| build(svg, true)).collect());
        if let Some(Some(geometry)) = icons.get(index) {
            paint_geometry_optical(painter, rect, geometry, color, [24.0, 24.0]);
        }
        return true;
    }
    let image_action = match icon {
        super::Icon::ImageCrop => Some(0),
        super::Icon::ImageSelectArea => Some(1),
        super::Icon::ImageRemoveBackground => Some(2),
        super::Icon::ImageEditPrompt => Some(3),
        super::Icon::ImageExpand => Some(4),
        super::Icon::ImageEnhance => Some(5),
        super::Icon::ImageVectorize => Some(6),
        _ => None,
    };
    if let Some(index) = image_action {
        static ICONS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
        let icons = ICONS.get_or_init(|| IMAGE_ACTION_SVGS.iter().map(|svg| build(svg, true)).collect());
        if let Some(Some(geometry)) = icons.get(index) {
            paint_geometry_optical(painter, rect, geometry, color, [24.0, 24.0]);
        }
        return true;
    }
    let dialog_icon = match icon {
        super::Icon::TypeDialogJustify => Some(0),
        super::Icon::TypeDialogNone => Some(1),
        super::Icon::TypeDialogUnderline => Some(2),
        super::Icon::TypeDialogStrike => Some(3),
        super::Icon::TypeDialogUpper => Some(4),
        super::Icon::TypeDialogLower => Some(5),
        super::Icon::TypeDialogTitle => Some(6),
        super::Icon::TypeDialogSmallCaps => Some(7),
        super::Icon::TypeDialogTrimStandard => Some(8),
        super::Icon::TypeDialogTrimCap => Some(9),
        super::Icon::TypeDialogListBullets => Some(10),
        super::Icon::TypeDialogListNumbered => Some(11),
        super::Icon::TypeDialogTruncate => Some(12),
        _ => None,
    };
    if let Some(index) = dialog_icon {
        static ICONS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
        let icons = ICONS.get_or_init(|| TYPOGRAPHY_DIALOG_SVGS.iter().map(|svg| build(svg, true)).collect());
        if let Some(Some(geometry)) = icons.get(index) {
            paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
        }
        return true;
    }
    let typography = match icon {
        super::Icon::TypeLineHeight => Some(0),
        super::Icon::TypeLetterSpacing => Some(1),
        super::Icon::TypeAlignLeft => Some(2),
        super::Icon::TypeAlignCenter => Some(3),
        super::Icon::TypeAlignRight => Some(4),
        super::Icon::TypeAlignTop => Some(5),
        super::Icon::TypeAlignMiddle => Some(6),
        super::Icon::TypeAlignBottom => Some(7),
        super::Icon::TypeSettings => Some(8),
        _ => None,
    };
    if let Some(index) = typography {
        static ICONS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
        let icons = ICONS.get_or_init(|| TYPOGRAPHY_SVGS.iter().map(|svg| build(svg, true)).collect());
        if let Some(Some(geometry)) = icons.get(index) {
            paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
        }
        return true;
    }
    let direction = match icon {
        super::Icon::Chevron => Some(ChevronDirection::Down),
        super::Icon::ChevronRight => Some(ChevronDirection::Right),
        super::Icon::ChevronUp => Some(ChevronDirection::Up),
        super::Icon::ChevronLeft => Some(ChevronDirection::Left),
        _ => None,
    };
    if let Some(direction) = direction {
        paint_chevron(painter, rect.center(), rect.width().min(rect.height()).min(16.0) * 0.78, direction, color);
        return true;
    }
    let Some(index) = mapping::index(icon) else { return false };
    let Some(geometry) = cached(index, false) else { return false };
    paint_geometry_optical(painter, rect, &geometry, color, [24.0, 24.0]);
    true
}

pub(super) fn paint_compact_chevron(painter: &Painter, rect: Rect, color: Color32) {
    paint_chevron(painter, rect.center(), rect.width().min(rect.height()) * 0.78, ChevronDirection::Down, color);
}

pub(super) fn paint_home_tab(painter: &Painter, rect: Rect, color: Color32) {
    static GEOMETRY: OnceLock<Option<Geometry>> = OnceLock::new();
    if let Some(geometry) = GEOMETRY.get_or_init(|| build(HOME_TAB_SVG, true)) {
        paint_geometry_optical(painter, rect, geometry, color, [24.0, 24.0]);
    }
}

pub(super) fn paint_settings_tab(painter: &Painter, rect: Rect, color: Color32) {
    static GEOMETRY: OnceLock<Option<Geometry>> = OnceLock::new();
    if let Some(geometry) = GEOMETRY.get_or_init(|| build(SETTINGS_TAB_SVG, true)) {
        paint_geometry_optical(painter, rect, geometry, color, [20.0, 20.0]);
    }
}

pub(super) fn paint_tab_document(painter: &Painter, rect: Rect, color: Color32) {
    // egui meshes have hard triangle edges. Rasterize this small SVG at twice the display
    // resolution so its curves and diagonal remain smooth after texture filtering.
    let pixels = (rect.width() * painter.ctx().pixels_per_point().clamp(1.0, 4.0) * 2.0).round() as u32;
    let cache = painter.ctx().data_mut(|data| data.get_temp_mut_or_default::<TabDocumentTextures>(egui::Id::new("tab_document_textures")).clone());
    let existing = cache.0.lock().ok().and_then(|textures| textures.get(&pixels).cloned());
    let texture = existing.or_else(|| {
        let rgba = raster_tab_document(pixels)?;
        let image = egui::ColorImage::from_rgba_unmultiplied([pixels as usize, pixels as usize], &rgba);
        let texture = painter.ctx().load_texture("tab-document", image, egui::TextureOptions::LINEAR);
        cache.0.lock().ok()?.insert(pixels, texture.clone());
        Some(texture)
    });
    if let Some(texture) = texture {
        painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    }
}

fn raster_tab_document(pixels: u32) -> Option<Vec<u8>> {
    if !(1..=256).contains(&pixels) {
        return None;
    }
    let tree = usvg::Tree::from_str(TAB_DOCUMENT_SVG, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(pixels, pixels)?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let scale = pixels as f32 / 19.0;
    for node in tree.root().children() {
        if let usvg::Node::Path(source) = node
            && let Some(fill) = source.fill()
            && let Some(path) = source.data().clone().transform(source.abs_transform())
        {
            let rule = match fill.rule() {
                usvg::FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
                usvg::FillRule::NonZero => tiny_skia::FillRule::Winding,
            };
            pixmap.fill_path(&path, &paint, rule, tiny_skia::Transform::from_scale(scale, scale), None);
        }
    }
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.data().chunks_exact(4) {
        rgba.extend_from_slice(&[255, 255, 255, pixel[3]]);
    }
    Some(rgba)
}

pub(super) fn paint_header(painter: &Painter, rect: Rect, icon: super::HeaderIcon, color: Color32) {
    if icon == super::HeaderIcon::ZoomChevron {
        paint_chevron(painter, rect.center(), 7.8, ChevronDirection::Down, color);
        return;
    }
    static HEADER: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = HEADER.get_or_init(|| HEADER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::HeaderIcon::CollapsePanel => 0,
        super::HeaderIcon::Present => 1,
        super::HeaderIcon::ZoomChevron => 2,
        super::HeaderIcon::AvatarRing => 3,
        super::HeaderIcon::CreateComponent => 4,
        super::HeaderIcon::Mask => 5,
        super::HeaderIcon::MaskTertiary => 6,
        super::HeaderIcon::Union => 7,
        super::HeaderIcon::UnionTertiary => 8,
        super::HeaderIcon::EditObject => 9,
        super::HeaderIcon::SelectMatching => 10,
        super::HeaderIcon::TextLink => 11,
        super::HeaderIcon::TextVariable => 12,
        super::HeaderIcon::More => 13,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        if matches!(icon, super::HeaderIcon::ZoomChevron | super::HeaderIcon::CollapsePanel) {
            paint_geometry(painter, rect, geometry, color, HEADER_SVGS[index].1);
        } else {
            paint_geometry_optical(painter, rect, geometry, color, HEADER_SVGS[index].1);
        }
    }
}

pub(super) fn paint_position(painter: &Painter, rect: Rect, icon: super::PositionIcon, color: Color32, tertiary: Color32) {
    static POSITION: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = POSITION.get_or_init(|| POSITION_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = match icon {
        super::PositionIcon::AlignLeft => 0,
        super::PositionIcon::AlignHCenter => 1,
        super::PositionIcon::AlignRight => 2,
        super::PositionIcon::AlignTop => 3,
        super::PositionIcon::AlignVCenter => 4,
        super::PositionIcon::AlignBottom => 5,
        super::PositionIcon::Rotation => 6,
        super::PositionIcon::RotateRight => 7,
        super::PositionIcon::FlipH => 8,
        super::PositionIcon::FlipV => 9,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry_fitted(painter, rect, geometry, color, Some(tertiary), [24.0, 24.0], true);
    }
}

pub(super) fn paint_vector_point(painter: &Painter, rect: Rect, icon: super::VectorPointIcon, color: Color32) {
    static POINT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = POINT.get_or_init(|| VECTOR_POINT_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = match icon {
        super::VectorPointIcon::MirrorNone => 0,
        super::VectorPointIcon::MirrorAngle => 1,
        super::VectorPointIcon::MirrorAngleAndLength => 2,
        super::VectorPointIcon::More => 3,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry_fitted(painter, rect, geometry, color, None, [24.0, 24.0], true);
    }
}

pub(super) fn paint_layout(painter: &Painter, rect: Rect, icon: super::LayoutIcon, color: Color32) {
    static LAYOUT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = LAYOUT.get_or_init(|| LAYOUT_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::LayoutIcon::AspectRatio => 0,
        super::LayoutIcon::AutoLayout => 1,
        super::LayoutIcon::Free => 2,
        super::LayoutIcon::Vertical => 3,
        super::LayoutIcon::Horizontal => 4,
        super::LayoutIcon::Grid => 5,
        super::LayoutIcon::Wrap => 6,
        super::LayoutIcon::GapVertical => 7,
        super::LayoutIcon::Settings => 8,
        super::LayoutIcon::PaddingHorizontal => 9,
        super::LayoutIcon::PaddingVertical => 10,
        super::LayoutIcon::PaddingIndividual => 11,
        super::LayoutIcon::AutoLayoutActive => 12,
        super::LayoutIcon::PaddingIndividualActive => 13,
        super::LayoutIcon::PaddingLeft => 14,
        super::LayoutIcon::PaddingRight => 15,
        super::LayoutIcon::PaddingTop => 16,
        super::LayoutIcon::PaddingBottom => 17,
        super::LayoutIcon::TextAutoWidth => 18,
        super::LayoutIcon::TextAutoHeight => 19,
        super::LayoutIcon::TextFixed => 20,
        super::LayoutIcon::TextAspectRatio => 21,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        if matches!(
            icon,
            super::LayoutIcon::TextAutoWidth | super::LayoutIcon::TextAutoHeight | super::LayoutIcon::TextFixed | super::LayoutIcon::TextAspectRatio
        ) {
            paint_geometry(painter, rect, geometry, color, LAYOUT_SVGS[index].1);
        } else {
            paint_geometry_optical(painter, rect, geometry, color, LAYOUT_SVGS[index].1);
        }
    }
}

pub(super) fn paint_appearance(painter: &Painter, rect: Rect, icon: super::AppearanceIcon, color: Color32) {
    static APPEARANCE: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = APPEARANCE.get_or_init(|| APPEARANCE_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = match icon {
        super::AppearanceIcon::Visible => 0,
        super::AppearanceIcon::Blend => 1,
        super::AppearanceIcon::Opacity => 2,
        super::AppearanceIcon::Corners => 3,
        super::AppearanceIcon::CornerTopLeft => 4,
        super::AppearanceIcon::CornerTopRight => 5,
        super::AppearanceIcon::CornerBottomLeft => 6,
        super::AppearanceIcon::CornerBottomRight => 7,
        super::AppearanceIcon::CornerSettings => 8,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry_optical(painter, rect, geometry, color, [24.0, 24.0]);
    }
}

pub(super) fn paint_colorpicker(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    if index == 7 {
        paint_chevron(painter, rect.center(), 5.0, ChevronDirection::Down, color);
        return;
    }
    static COLORPICKER: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = COLORPICKER.get_or_init(|| COLORPICKER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, COLORPICKER_SVGS[index].1);
    }
}

/// Exact SVG paths supplied with the properties-panel design reference.
pub(super) fn paint_panel_reference(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    type PanelIcon = Option<(Arc<Geometry>, [f32; 2])>;
    static ICONS: OnceLock<Vec<PanelIcon>> = OnceLock::new();
    let icons = ICONS.get_or_init(|| {
        let entries: Vec<serde_json::Value> = serde_json::from_str(PANEL_REFERENCE).unwrap_or_default();
        let mut icons: Vec<PanelIcon> = Vec::with_capacity(entries.len());
        for entry in &entries {
            let icon = if let Some(alias) = entry.get("alias").and_then(serde_json::Value::as_u64).and_then(|index| usize::try_from(index).ok()) {
                icons.get(alias).cloned().flatten()
            } else {
                (|| {
                    let svg = entry.get("svg")?.as_str()?;
                    let size = entry.get("size")?.as_array()?;
                    let w = size.first()?.as_f64()? as f32;
                    let h = size.get(1)?.as_f64()? as f32;
                    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
                        return None;
                    }
                    build(svg, true).map(|geometry| (Arc::new(geometry), [w, h]))
                })()
            };
            icons.push(icon);
        }
        icons
    });
    if let Some(Some((geometry, size))) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, *size);
    }
}

pub(super) fn panel_reference_catalog() -> &'static [(usize, String)] {
    static CATALOG: OnceLock<Vec<(usize, String)>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let entries: Vec<serde_json::Value> = serde_json::from_str(PANEL_REFERENCE).unwrap_or_default();
        entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.get("alias").is_none())
            .filter_map(|(index, entry)| Some((index, entry.get("name")?.as_str()?.to_owned())))
            .collect()
    })
}

pub(super) fn paint_property(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static PROPERTY: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = PROPERTY.get_or_init(|| PROPERTY_SVGS.iter().map(|svg| build(svg, true)).collect());
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, [12.0, 12.0]);
    }
}

pub(super) fn paint_stroke_side(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static STROKE_SIDES: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = STROKE_SIDES.get_or_init(|| STROKE_SIDE_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    if let (Some(Some(geometry)), Some((_, size))) = (icons.get(index), STROKE_SIDE_SVGS.get(index)) {
        paint_geometry(painter, rect, geometry, color, *size);
    }
}

pub(super) fn paint_layer(painter: &Painter, rect: Rect, icon: super::LayerIcon, color: Color32) {
    let chevron = match icon {
        super::LayerIcon::ChevronDown => Some(ChevronDirection::Down),
        super::LayerIcon::ChevronRight => Some(ChevronDirection::Right),
        _ => None,
    };
    if let Some(direction) = chevron {
        paint_chevron(painter, rect.center(), rect.width().max(rect.height()), direction, color);
        return;
    }
    static LAYERS: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = LAYERS.get_or_init(|| LAYER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::LayerIcon::Frame => 0,
        super::LayerIcon::LayoutH => 1,
        super::LayerIcon::LayoutV => 2,
        super::LayerIcon::LayoutGrid => 3,
        super::LayerIcon::Text => 4,
        super::LayerIcon::Absolute => 5,
        super::LayerIcon::ChevronDown => 6,
        super::LayerIcon::ChevronRight => 7,
        super::LayerIcon::Mask => 8,
        super::LayerIcon::Ellipse => 9,
        super::LayerIcon::EllipseMask => 10,
        super::LayerIcon::Component => 11,
        super::LayerIcon::Instance => 12,
        super::LayerIcon::Vector => 13,
        super::LayerIcon::Image => 14,
        super::LayerIcon::Star => 15,
        super::LayerIcon::Polygon => 16,
        super::LayerIcon::Arrow => 17,
        super::LayerIcon::Line => 18,
        super::LayerIcon::Rectangle => 19,
        super::LayerIcon::Group => 20,
        super::LayerIcon::CollapseAll => 21,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, LAYER_SVGS[index].1);
    }
}

pub(super) fn paint_nav(painter: &Painter, rect: Rect, icon: super::NavIcon, color: Color32) {
    static NAV: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = NAV.get_or_init(|| NAV_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::NavIcon::Logo => 0,
        super::NavIcon::File => 1,
        super::NavIcon::Agent => 2,
        super::NavIcon::Assets => 3,
        super::NavIcon::Variables => 4,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        if icon == super::NavIcon::Logo {
            paint_geometry(painter, rect, geometry, color, NAV_SVGS[index].1);
        } else {
            paint_geometry_optical(painter, rect, geometry, color, NAV_SVGS[index].1);
        }
    }
}

pub(super) fn paint_agent(painter: &Painter, rect: Rect, icon: super::AgentIcon, color: Color32) {
    let chevron = match icon {
        super::AgentIcon::Back => Some((ChevronDirection::Left, 9.0)),
        super::AgentIcon::Chevron => Some((ChevronDirection::Down, rect.width().max(rect.height()))),
        super::AgentIcon::Down => Some((ChevronDirection::Down, rect.width().min(rect.height()) * 0.57)),
        super::AgentIcon::Up => Some((ChevronDirection::Up, rect.width().min(rect.height()) * 0.57)),
        _ => None,
    };
    if let Some((direction, painted_size)) = chevron {
        paint_chevron(painter, rect.center(), painted_size, direction, color);
        return;
    }
    static AGENT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = AGENT.get_or_init(|| AGENT_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::AgentIcon::Back => 0,
        super::AgentIcon::More => 1,
        super::AgentIcon::Emblem => 2,
        super::AgentIcon::Polish => 3,
        super::AgentIcon::Generate => 4,
        super::AgentIcon::Plugins => 5,
        super::AgentIcon::Chevron => 6,
        super::AgentIcon::Add => 7,
        super::AgentIcon::Library => 8,
        super::AgentIcon::Microphone => 9,
        super::AgentIcon::Send => 10,
        super::AgentIcon::Paperclip => 11,
        super::AgentIcon::Lightning => 12,
        super::AgentIcon::Grid => 13,
        super::AgentIcon::Down => 14,
        super::AgentIcon::Up => 15,
        super::AgentIcon::SendUp => 16,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, AGENT_SVGS[index].1);
    }
}

pub(super) fn paint_sizing(painter: &Painter, rect: Rect, icon: super::SizingIcon, color: Color32) {
    static SIZING: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = SIZING.get_or_init(|| SIZING_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::SizingIcon::Fixed => 0,
        super::SizingIcon::Hug => 1,
        super::SizingIcon::Fill => 2,
        super::SizingIcon::Check => 3,
        super::SizingIcon::Minimum => 4,
        super::SizingIcon::Maximum => 5,
        super::SizingIcon::Variable => 6,
        super::SizingIcon::HeightFixed => 7,
        super::SizingIcon::HeightHug => 8,
        super::SizingIcon::HeightFill => 9,
        super::SizingIcon::HeightMinimum => 10,
        super::SizingIcon::HeightMaximum => 11,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry_optical(painter, rect, geometry, color, SIZING_SVGS[index].1);
    }
}

pub(super) fn paint_alignment(painter: &Painter, rect: Rect, horizontal: bool, position: usize) {
    static ALIGNMENT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = ALIGNMENT.get_or_init(|| ALIGNMENT_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = (if horizontal { 3 } else { 0 }) + position.min(2);
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, crate::theme::Tokens::get(painter.ctx()).accent_text, [16.0, 16.0]);
    }
}

fn paint_geometry(painter: &Painter, rect: Rect, geometry: &Geometry, color: Color32, view_box: [f32; 2]) {
    paint_geometry_fitted(painter, rect, geometry, color, None, view_box, false);
}

fn paint_geometry_optical(painter: &Painter, rect: Rect, geometry: &Geometry, color: Color32, view_box: [f32; 2]) {
    paint_geometry_fitted(painter, rect, geometry, color, None, view_box, true);
}

fn paint_geometry_fitted(
    painter: &Painter,
    rect: Rect,
    geometry: &Geometry,
    color: Color32,
    tertiary: Option<Color32>,
    view_box: [f32; 2],
    optical: bool,
) {
    if !rect.is_positive() || !view_box[0].is_finite() || !view_box[1].is_finite() || view_box[0] <= 0.0 || view_box[1] <= 0.0 {
        return;
    }
    let (scale, origin) = fit_geometry(rect, geometry, view_box, optical);
    for (index, part) in geometry.0.iter().enumerate() {
        let tint = if index == 1 { tertiary.unwrap_or(color) } else { color }.gamma_multiply(part.opacity);
        if rect.width().max(rect.height()) <= 28.0 && paint_raster_part(painter, rect, geometry, index, part, view_box, optical, tint) {
            continue;
        }
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(part.positions.len());
        for &[x, y] in &part.positions {
            mesh.vertices.push(egui::epaint::Vertex::untextured(pos2(origin.x + x * scale, origin.y + y * scale), tint));
        }
        mesh.indices.extend_from_slice(&part.indices);
        painter.add(Shape::mesh(mesh));
    }
}

/// Fit ordinary UI glyphs by their painted bounds. This leaves equal optical space around a
/// 10-unit layer icon and a 24-unit panel icon without stretching either one's geometry.
fn fit_geometry(rect: Rect, geometry: &Geometry, view_box: [f32; 2], optical: bool) -> (f32, egui::Pos2) {
    if optical {
        let mut min = pos2(f32::INFINITY, f32::INFINITY);
        let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for part in &geometry.0 {
            for &[x, y] in &part.positions {
                min.x = min.x.min(x);
                min.y = min.y.min(y);
                max.x = max.x.max(x);
                max.y = max.y.max(y);
            }
        }
        let extent = (max.x - min.x).max(max.y - min.y);
        if extent.is_finite() && extent > 0.0 {
            let scale = rect.width().min(rect.height()) * 0.78 / extent;
            return (scale, rect.center() - vec2((min.x + max.x) * 0.5 * scale, (min.y + max.y) * 0.5 * scale));
        }
    }
    let scale = (rect.width() / view_box[0]).min(rect.height() / view_box[1]);
    (scale, rect.center() - vec2(view_box[0] * scale * 0.5, view_box[1] * scale * 0.5))
}

/// Small tessellated icons have hard mesh edges in egui. Rasterize each part at twice the
/// display resolution once, then tint the cached alpha mask with the current theme color.
fn paint_raster_part(
    painter: &Painter,
    rect: Rect,
    geometry: &Geometry,
    index: usize,
    part: &Part,
    view_box: [f32; 2],
    optical: bool,
    tint: Color32,
) -> bool {
    let pixels_per_point = painter.ctx().pixels_per_point().clamp(1.0, 4.0) * 2.0;
    let w = (rect.width() * pixels_per_point).round().clamp(1.0, 192.0) as u16;
    let h = (rect.height() * pixels_per_point).round().clamp(1.0, 192.0) as u16;
    let key = (geometry as *const Geometry as usize, index, w, h, optical);
    let cache = painter.ctx().data_mut(|data| data.get_temp_mut_or_default::<IconTextures>(egui::Id::new("lizard_icon_textures")).clone());
    let existing = cache.0.lock().ok().and_then(|textures| textures.get(&key).cloned());
    let texture = existing.or_else(|| {
        let pixels = raster_geometry_part(geometry, part, view_box, optical, w, h)?;
        let image = egui::ColorImage::from_rgba_unmultiplied([usize::from(w), usize::from(h)], &pixels);
        let texture = painter.ctx().load_texture(format!("lizard-icon-{}-{index}-{w}-{h}-{optical}", key.0), image, egui::TextureOptions::LINEAR);
        cache.0.lock().ok()?.insert(key, texture.clone());
        Some(texture)
    });
    if let Some(texture) = texture {
        painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        return true;
    }
    false
}

fn raster_geometry_part(geometry: &Geometry, part: &Part, view_box: [f32; 2], optical: bool, w: u16, h: u16) -> Option<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(u32::from(w), u32::from(h))?;
    let pixel_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(f32::from(w), f32::from(h)));
    let (scale, origin) = fit_geometry(pixel_rect, geometry, view_box, optical);
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let mut path = tiny_skia::PathBuilder::new();
    for triangle in part.indices.chunks_exact(3) {
        let [Some(&a), Some(&b), Some(&c)] = [
            triangle.first().and_then(|i| part.positions.get(*i as usize)),
            triangle.get(1).and_then(|i| part.positions.get(*i as usize)),
            triangle.get(2).and_then(|i| part.positions.get(*i as usize)),
        ] else {
            continue;
        };
        path.move_to(origin.x + a[0] * scale, origin.y + a[1] * scale);
        path.line_to(origin.x + b[0] * scale, origin.y + b[1] * scale);
        path.line_to(origin.x + c[0] * scale, origin.y + c[1] * scale);
        path.close();
    }
    if let Some(path) = path.finish() {
        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.data().chunks_exact(4) {
        rgba.extend_from_slice(&[255, 255, 255, pixel[3]]);
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_reference_svgs_have_vector_geometry() {
        for (index, (svg, _)) in AGENT_SVGS.into_iter().enumerate() {
            if matches!(index, 0 | 6 | 14 | 15) {
                continue;
            }
            assert!(build(svg, true).is_some(), "Agent SVG {index} did not tessellate");
        }
    }

    #[test]
    fn tab_document_has_antialiased_edges() {
        let rgba = raster_tab_document(56).expect("tab document SVG rasterizes");
        assert!(rgba.chunks_exact(4).any(|pixel| (1..255).contains(&pixel[3])));
    }

    #[test]
    fn sizing_menu_svgs_have_vector_geometry() {
        for (index, (svg, _)) in SIZING_SVGS.into_iter().enumerate() {
            let Some(geometry) = build(svg, true) else { panic!("sizing menu SVG {index} has no geometry") };
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn alignment_svgs_have_vector_geometry() {
        for (index, svg) in ALIGNMENT_SVGS.into_iter().enumerate() {
            let Some(geometry) = build(svg, true) else { panic!("alignment SVG {index} has no geometry") };
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn every_ui_icon_has_vector_geometry() {
        for (index, svg) in IMAGE_ACTION_SVGS.iter().enumerate() {
            let Some(geometry) = build(svg, true) else { panic!("image action SVG {index} has no geometry") };
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()), "image action SVG {index} has an empty mesh");
        }
        for &icon in super::super::Icon::ALL {
            if matches!(
                icon,
                super::super::Icon::Logo
                    | super::super::Icon::Angle
                    | super::super::Icon::Chevron
                    | super::super::Icon::ChevronRight
                    | super::super::Icon::ChevronLeft
                    | super::super::Icon::ChevronUp
            ) {
                continue;
            }
            let Some(index) = mapping::index(icon) else {
                let ctx = egui::Context::default();
                let mut output = ctx.run_ui(Default::default(), |ui| {
                    super::super::paint(ui.painter(), Rect::from_min_size(pos2(0.0, 0.0), vec2(24.0, 24.0)), icon, Color32::WHITE);
                });
                output.textures_delta.clear();
                assert!(!output.shapes.is_empty(), "{icon:?} has no UI3, custom SVG or painted geometry");
                continue;
            };
            let geometry = cached(index, false).unwrap_or_else(|| panic!("{icon:?} has no geometry"));
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()), "{icon:?} has an empty mesh");
        }
    }

    #[test]
    fn typography_svgs_have_vector_geometry() {
        for (index, svg) in TYPOGRAPHY_SVGS.iter().chain(TYPOGRAPHY_DIALOG_SVGS.iter()).enumerate() {
            assert!(build(svg, true).is_some(), "typography SVG {index} has no geometry");
        }
    }

    #[test]
    fn disclosure_chevrons_share_one_svg() {
        let source = build(CHEVRON_SVG, true).expect("shared chevron has geometry");
        assert_eq!(source.0.len(), 1);
        assert!(!source.0[0].indices.is_empty());
        assert!(source.0[0].positions.iter().flatten().all(|coordinate| coordinate.is_finite()));
        for index in [61, 63, 65, 211] {
            assert!(sources().get(index).is_some_and(String::is_empty));
        }
    }

    #[test]
    fn toolbar_icons_use_their_original_svg_paths() {
        for svg in TOOLBAR_SVGS.into_iter().filter(|svg| !svg.is_empty()) {
            let geometry = build(svg, true).expect("toolbar icon has valid SVG geometry");
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn header_icons_use_their_original_svg_paths() {
        for (svg, _) in HEADER_SVGS.into_iter().filter(|(svg, _)| !svg.is_empty()) {
            let geometry = build(svg, true).expect("header icon has valid SVG geometry");
            assert!(geometry.0.iter().all(|part| part.opacity == 1.0));
        }
    }

    #[test]
    fn position_icons_keep_svg_paths_and_secondary_alignment_guides() {
        for (index, svg) in POSITION_SVGS.iter().enumerate() {
            let geometry = build(svg, true).expect("position icon has valid SVG geometry");
            assert_eq!(geometry.0.len(), if index < 6 { 2 } else { 1 });
        }
    }

    #[test]
    fn vector_point_icons_from_the_panel_render_as_geometry() {
        for svg in VECTOR_POINT_SVGS {
            let geometry = build(svg, true).expect("vector point SVG has valid geometry");
            assert!(geometry.0.iter().any(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn appearance_reference_svgs_have_renderable_paths() {
        for (index, svg) in APPEARANCE_SVGS.iter().enumerate() {
            let geometry = build(svg, true).unwrap_or_else(|| panic!("appearance SVG {index} has no geometry"));
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()));
        }
    }

    #[test]
    fn layout_reference_svgs_have_renderable_paths() {
        for (svg, _) in LAYOUT_SVGS {
            assert!(build(svg, true).is_some(), "layout SVG did not produce vector geometry");
        }
    }

    #[test]
    fn panel_reference_svgs_have_renderable_paths() {
        let entries: Vec<serde_json::Value> = serde_json::from_str(PANEL_REFERENCE).expect("panel SVG catalog is valid JSON");
        assert_eq!(entries.len(), 78);
        assert_eq!(panel_reference_catalog().len(), 27);
        for (index, entry) in entries.iter().enumerate() {
            if let Some(alias) = entry["alias"].as_u64() {
                assert!((alias as usize) < index, "alias must point to an earlier drawing");
                continue;
            }
            let svg = entry["svg"].as_str().expect("SVG source");
            let size = entry["size"].as_array().expect("SVG dimensions");
            assert!(size.iter().all(|axis| axis.as_f64().is_some_and(|n| n.is_finite() && n > 0.0)));
            assert!(entry["name"].as_str().is_some_and(|name| !name.is_empty()));
            // The four gradient illustration layers are rendered together into the bundled
            // Glass light preview, since this path tessellator paints solid colours only.
            if (73..=76).contains(&index) {
                continue;
            }
            assert!(build(svg, true).is_some(), "panel SVG {index} did not produce vector geometry");
        }
    }
}
