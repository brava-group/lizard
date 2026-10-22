//! Build the app icon from the same vector mark used in Lizard's navigation panel.
//! `resvg` rasterises the macOS icon inside its transparent canvas, a rounded fallback,
//! and the vector layer compiled by Apple's Icon Composer tool.

use std::path::Path;
use std::process::Command;

const MARK: &str = "crates/ui-egui/assets/nav-logo.svg";

fn placed_mark(mark: &str, x: u32, y: u32, width: u32, height: u32) -> String {
    mark.replacen("<svg ", &format!("<svg x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" "), 1).replace("fill=\"#fff\"", "fill=\"#000\"")
}

fn icon_svg(mark: &str, padded: bool) -> Option<String> {
    if !mark.starts_with("<svg ") || !mark.contains("fill=\"#fff\"") {
        return None;
    }
    let mark = placed_mark(mark, 82, 66, 348, 380);
    let tile = format!("<rect width=\"512\" height=\"512\" rx=\"112\" fill=\"#fff\"/>{mark}");
    let content = if padded {
        // A legacy .icns is not masked by macOS. Keep the 824/1024 icon body inside a
        // transparent canvas so its shape and apparent Dock size match native icons.
        format!("<g transform=\"translate(50 50) scale(0.8046875)\">{tile}</g>")
    } else {
        tile
    };
    Some(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 512 512\">{content}</svg>"))
}

fn render(svg: &Path, size: u32, out: &Path) -> bool {
    let s = size.to_string();
    let ok = Command::new("resvg").args(["-w", &s, "-h", &s]).arg(svg).arg(out).status().is_ok_and(|st| st.success());
    if !ok {
        eprintln!("icon: resvg failed for {} (is it installed? `cargo install resvg`)", out.display());
    }
    ok
}

pub fn generate(root: &Path) -> bool {
    let dir = root.join("assets/app-icon");
    let Ok(mark) = std::fs::read_to_string(root.join(MARK)) else {
        eprintln!("icon: cannot read {MARK}");
        return false;
    };
    let (Some(mac), Some(tile)) = (icon_svg(&mark, true), icon_svg(&mark, false)) else {
        eprintln!("icon: unexpected SVG markup in {MARK}");
        return false;
    };
    let tmp = root.join("target/icon");
    let mac_svg = dir.join("figcraft.svg");
    let tile_svg = dir.join("figcraft-small.svg");
    let glass_svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1024 1024\">{}</svg>", placed_mark(&mark, 164, 132, 696, 760));
    let glass_dir = dir.join("Lizard.icon/Assets");
    let mut ok = std::fs::create_dir_all(&tmp).is_ok()
        && std::fs::create_dir_all(&glass_dir).is_ok()
        && std::fs::write(&mac_svg, mac).is_ok()
        && std::fs::write(&tile_svg, tile).is_ok()
        && std::fs::write(glass_dir.join("Mark.svg"), glass_svg).is_ok();
    if !ok {
        eprintln!("icon: cannot write {}", tmp.display());
        return false;
    }
    for size in [16u32, 32, 48, 64, 128, 256, 512, 1024] {
        ok &= render(&mac_svg, size, &dir.join(format!("figcraft-{size}.png")));
        if size <= 512 {
            let hd = dir.join(format!("hicolor/{size}x{size}/apps"));
            let _ = std::fs::create_dir_all(&hd);
            ok &= render(&tile_svg, size, &hd.join("ai.storyteller.figcraft.png"));
        }
    }
    // Windows .ico (PNG-compressed entries).
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let pngs: Vec<Vec<u8>> = sizes
        .iter()
        .filter_map(|s| {
            let png = tmp.join(format!("ico-{s}.png"));
            render(&tile_svg, *s, &png).then(|| std::fs::read(png).ok()).flatten()
        })
        .collect();
    ok &= pngs.len() == sizes.len();
    let mut ico = vec![0, 0, 1, 0, pngs.len() as u8, 0];
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (s, png) in sizes.iter().zip(&pngs) {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        ico.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &pngs {
        ico.extend_from_slice(png);
    }
    ok &= std::fs::write(dir.join("figcraft.ico"), ico).is_ok();
    // Linux uses the rounded fallback as its scalable icon.
    let sd = dir.join("hicolor/scalable/apps");
    let _ = std::fs::create_dir_all(&sd);
    ok &= std::fs::copy(&tile_svg, sd.join("ai.storyteller.figcraft.svg")).is_ok();
    eprintln!("icon: wrote {}", dir.display());
    ok
}
