//! UI3 SVG paths tessellated into egui meshes. No icon bitmaps or textures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Color32, Mesh, Painter, Rect, Shape, pos2};
use lyon_tessellation::path::{Path, math::point};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers,
};
use usvg::tiny_skia_path::PathSegment;

#[path = "ui3_mapping.rs"]
mod mapping;

const VECTORS: &str = include_str!("../assets/ui3-vectors.json");
const TAB_DOCUMENT_SVG: &str = include_str!("../assets/tab-document.svg");
const MODE_SVGS: [&str; 4] = [
    include_str!("../assets/mode-draw.svg"),
    include_str!("../assets/mode-design.svg"),
    include_str!("../assets/mode-motion.svg"),
    include_str!("../assets/mode-dev.svg"),
];
const LAYER_SVGS: [(&str, [f32; 2]); 8] = [
    (include_str!("../assets/layer-frame.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-layout-horizontal.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-layout-vertical.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-layout-grid.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-text.svg"), [10.0, 10.0]),
    (include_str!("../assets/layer-absolute.svg"), [16.0, 16.0]),
    (include_str!("../assets/layer-chevron-down.svg"), [5.237, 3.122]),
    (include_str!("../assets/layer-chevron-right.svg"), [3.121, 5.243]),
];
const NAV_SVGS: [(&str, [f32; 2]); 5] = [
    (include_str!("../assets/nav-logo.svg"), [85.84124, 94.0]),
    (include_str!("../assets/nav-file.svg"), [10.0, 14.0]),
    (include_str!("../assets/nav-agent.svg"), [14.0, 14.0]),
    (include_str!("../assets/nav-assets.svg"), [14.0, 14.0]),
    (include_str!("../assets/nav-variables.svg"), [12.0, 13.23847]),
];
const HEADER_SVGS: [(&str, [f32; 2]); 10] = [
    (include_str!("../assets/header-present.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-zoom-chevron.svg"), [16.0, 16.0]),
    (include_str!("../assets/header-avatar-ring.svg"), [30.0, 40.0]),
    (include_str!("../assets/header-create-component.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-mask.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-mask-tertiary.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-union.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-union-tertiary.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-edit-object.svg"), [24.0, 24.0]),
    (include_str!("../assets/header-select-matching.svg"), [24.0, 24.0]),
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
    (include_str!("../assets/colorpicker-chevron.svg"), [5.001_525, 3.0017872]),
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
const LAYOUT_SVGS: [(&str, [f32; 2]); 18] = [
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
    (include_str!("../assets/agent-back.svg"), [5.001787, 9.001525]),
    (include_str!("../assets/agent-more.svg"), [15.0, 3.0]),
    (include_str!("../assets/agent-emblem.svg"), [16.499786, 16.499025]),
    (include_str!("../assets/agent-polish.svg"), [13.498501, 12.998539]),
    (include_str!("../assets/nav-agent.svg"), [14.0, 14.0]),
    (include_str!("../assets/agent-plugins.svg"), [14.0, 13.0]),
    (include_str!("../assets/agent-chevron.svg"), [9.001525, 5.001787]),
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
    (
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 13.99993896484375 14" preserveAspectRatio="xMidYMid meet"><path d="M3.34619 4.68945q-0.25293 0.08545-0.37939 0.30762-0.04102 0.08545-0.04102 0.25293 0 0.16748 0.04785 0.25977 0.05127 0.08887 1.8628 1.9038 1.81494 1.81152 1.9038 1.8628 0.09229 0.04785 0.25977 0.04785 0.16748 0 0.25635-0.04785 0.09229-0.05127 1.90381-1.8628 1.81494-1.81494 1.86279-1.9038 0.05127-0.09229 0.05127-0.25977 0-0.16748-0.04102-0.25293-0.09912-0.16748-0.28027-0.2666-0.05811-0.02734-0.09912-0.04102-0.04102-0.01367-0.15381-0.01367-0.11279 0-0.15381 0.01367-0.04101 0.01367-0.11279 0.04102-0.09912 0.05811-1.66455 1.62695l-1.56885 1.56543-2.82666-2.81299q-0.31104-0.29395-0.42041-0.37939-0.08545-0.05469-0.19824-0.05469l-0.02735 0q-0.14014 0-0.18115 0.01367z" fill="#777777"/></svg>"##,
        [13.999939, 14.0],
    ),
    (
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 13.99993896484375 14" preserveAspectRatio="xMidYMid meet"><path d="M6.84619 4.68945q-0.08545 0.02734-0.19482 0.1128-0.14014 0.12646-0.53321 0.51611l-1.28857 1.2749q-1.81836 1.82178-1.86279 1.90723-0.04102 0.08203-0.04102 0.23584 0 0.15381 0.03418 0.23926 0.0376 0.08203 0.1333 0.18115 0.09912 0.0957 0.18115 0.1333 0.08545 0.03418 0.23926 0.03418 0.15381 0 0.23584-0.04102 0.08545-0.04443 1.66797-1.62353l1.58252-1.58252 1.58252 1.58252q1.58252 1.5791 1.66455 1.62353 0.08545 0.04102 0.23926 0.04102 0.15381 0 0.23584-0.03418 0.08545-0.0376 0.18115-0.1333 0.09912-0.09912 0.1333-0.18115 0.0376-0.08545 0.0376-0.23926 0-0.15381-0.04443-0.23584-0.04101-0.08545-1.85938-1.90723l-1.44238-1.42871q-0.33496-0.33496-0.46143-0.41699-0.09912-0.07178-0.19824-0.07178l-0.04102 0q-0.14014 0-0.18115 0.01367z" fill="#858589"/></svg>"##,
        [13.999939, 14.0],
    ),
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

struct Part {
    positions: Vec<[f32; 2]>,
    indices: Vec<u32>,
    opacity: f32,
}

struct Geometry(Vec<Part>);

#[derive(Clone, Default)]
struct TabDocumentTextures(Arc<Mutex<HashMap<u32, egui::TextureHandle>>>);

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

pub(super) fn paint(painter: &Painter, rect: Rect, icon: super::Icon, color: Color32) -> bool {
    let Some(index) = mapping::index(icon) else { return false };
    // Keep the chevron's original SVG outline, including its one-unit stroke.
    let Some(geometry) = cached(index, icon == super::Icon::Chevron) else { return false };
    paint_geometry(painter, rect, &geometry, color, [24.0, 24.0]);
    true
}

pub(super) fn paint_mode(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static MODES: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let modes = MODES.get_or_init(|| MODE_SVGS.iter().map(|svg| build(svg, true)).collect());
    if let Some(Some(geometry)) = modes.get(index) {
        paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
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
    static HEADER: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = HEADER.get_or_init(|| HEADER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    let index = match icon {
        super::HeaderIcon::Present => 0,
        super::HeaderIcon::ZoomChevron => 1,
        super::HeaderIcon::AvatarRing => 2,
        super::HeaderIcon::CreateComponent => 3,
        super::HeaderIcon::Mask => 4,
        super::HeaderIcon::MaskTertiary => 5,
        super::HeaderIcon::Union => 6,
        super::HeaderIcon::UnionTertiary => 7,
        super::HeaderIcon::EditObject => 8,
        super::HeaderIcon::SelectMatching => 9,
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, HEADER_SVGS[index].1);
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
        paint_geometry_colored(painter, rect, geometry, color, Some(tertiary), [24.0, 24.0]);
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
    };
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, LAYOUT_SVGS[index].1);
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
        paint_geometry(painter, rect, geometry, color, [24.0, 24.0]);
    }
}

pub(super) fn paint_colorpicker(painter: &Painter, rect: Rect, index: usize, color: Color32) {
    static COLORPICKER: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = COLORPICKER.get_or_init(|| COLORPICKER_SVGS.iter().map(|(svg, _)| build(svg, true)).collect());
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, color, COLORPICKER_SVGS[index].1);
    }
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
        paint_geometry(painter, rect, geometry, color, NAV_SVGS[index].1);
    }
}

pub(super) fn paint_agent(painter: &Painter, rect: Rect, icon: super::AgentIcon, color: Color32) {
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
        paint_geometry(painter, rect, geometry, color, SIZING_SVGS[index].1);
    }
}

pub(super) fn paint_alignment(painter: &Painter, rect: Rect, horizontal: bool, position: usize) {
    static ALIGNMENT: OnceLock<Vec<Option<Geometry>>> = OnceLock::new();
    let icons = ALIGNMENT.get_or_init(|| ALIGNMENT_SVGS.iter().map(|svg| build(svg, true)).collect());
    let index = (if horizontal { 3 } else { 0 }) + position.min(2);
    if let Some(Some(geometry)) = icons.get(index) {
        paint_geometry(painter, rect, geometry, Color32::from_rgb(0x7c, 0xc4, 0xf8), [16.0, 16.0]);
    }
}

fn paint_geometry(painter: &Painter, rect: Rect, geometry: &Geometry, color: Color32, view_box: [f32; 2]) {
    paint_geometry_colored(painter, rect, geometry, color, None, view_box);
}

fn paint_geometry_colored(painter: &Painter, rect: Rect, geometry: &Geometry, color: Color32, tertiary: Option<Color32>, view_box: [f32; 2]) {
    let scale = (rect.width() / view_box[0]).min(rect.height() / view_box[1]);
    let origin = rect.center() - egui::vec2(view_box[0] * scale / 2.0, view_box[1] * scale / 2.0);
    for (index, part) in geometry.0.iter().enumerate() {
        let mut mesh = Mesh::default();
        mesh.vertices.reserve(part.positions.len());
        let tint = if index == 1 { tertiary.unwrap_or(color) } else { color }.gamma_multiply(part.opacity);
        for &[x, y] in &part.positions {
            mesh.vertices.push(egui::epaint::Vertex::untextured(pos2(origin.x + x * scale, origin.y + y * scale), tint));
        }
        mesh.indices.extend_from_slice(&part.indices);
        painter.add(Shape::mesh(mesh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_reference_svgs_have_vector_geometry() {
        for (index, (svg, _)) in AGENT_SVGS.into_iter().enumerate() {
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
        for &icon in super::super::Icon::ALL {
            if icon == super::super::Icon::Logo {
                continue;
            }
            let index = mapping::index(icon).unwrap();
            let geometry = cached(index, icon == super::super::Icon::Chevron).unwrap_or_else(|| panic!("{icon:?} has no geometry"));
            assert!(geometry.0.iter().all(|part| !part.indices.is_empty()), "{icon:?} has an empty mesh");
        }
    }

    #[test]
    fn mode_icons_use_their_original_svg_paths() {
        for svg in MODE_SVGS {
            let geometry = build(svg, true).expect("mode icon has valid SVG geometry");
            assert!(geometry.0.iter().all(|part| part.opacity == 1.0));
        }
    }

    #[test]
    fn header_icons_use_their_original_svg_paths() {
        for (svg, _) in HEADER_SVGS {
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
}
