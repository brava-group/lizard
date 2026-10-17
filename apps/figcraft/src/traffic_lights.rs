//! Align AppKit's native window controls with the 40-point document tab strip.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSWindowButton};
use objc2_foundation::NSPoint;

/// AppKit can reset these positions when a window changes display, size, or focus.
pub fn center_in_header() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    let Some(window) = application.keyWindow().or_else(|| application.mainWindow()) else {
        return;
    };
    let Some(content) = window.contentView() else {
        return;
    };
    let target_y = if content.isFlipped() {
        f64::from(figcraft_ui_egui::tabs::HEIGHT) / 2.0
    } else {
        content.bounds().size.height - f64::from(figcraft_ui_egui::tabs::HEIGHT) / 2.0
    };
    for kind in [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton] {
        let Some(button) = window.standardWindowButton(kind) else {
            continue;
        };
        let frame = button.frame();
        let local_center = NSPoint::new(button.bounds().size.width / 2.0, button.bounds().size.height / 2.0);
        let current_y = button.convertPoint_toView(local_center, Some(&content)).y;
        if (target_y - current_y).abs() < 0.25 {
            continue;
        }
        // The titlebar container can be flipped. Probe one point in its own coordinates so
        // AppKit's conversion supplies the direction without assuming its view hierarchy.
        button.setFrameOrigin(NSPoint::new(frame.origin.x, frame.origin.y + 1.0));
        let probe_y = button.convertPoint_toView(local_center, Some(&content)).y;
        button.setFrameOrigin(frame.origin);
        let direction = probe_y - current_y;
        if !direction.is_finite() || direction.abs() < 0.5 {
            continue;
        }
        let offset = ((target_y - current_y) / direction).clamp(-20.0, 20.0);
        if offset.is_finite() {
            button.setFrameOrigin(NSPoint::new(frame.origin.x, frame.origin.y + offset));
        }
    }
}
