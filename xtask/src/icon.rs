//! Render the owner's editable Lizard icon document for desktop app bundles.

use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use lizard_doc::NodeKind;
use lizard_render::{Pixmap, View};

const SOURCE: &str = "assets/app-icon/Lizard-new-icon.lizard";
const MASTER_SIZE: u32 = 1024;
const MASTER_FRAME: &str = "Lizard New Icon";

fn icon_png(root: &Path) -> Result<Vec<u8>, String> {
    let (doc, warnings) = lizard_format::load_file(&root.join(SOURCE)).map_err(|e| e.to_string())?;
    if !warnings.is_empty() {
        return Err(format!("icon source has import warnings: {}", warnings.join("; ")));
    }
    let page = doc.current_page.ok_or("icon source has no page")?;
    let frame = doc
        .node(page)
        .and_then(|page| {
            page.children.iter().filter_map(|id| doc.node(*id)).find(|node| node.name == MASTER_FRAME && matches!(node.kind, NodeKind::Frame))
        })
        .ok_or("icon source has no 'Lizard New Icon' frame")?;
    if (frame.width - f64::from(MASTER_SIZE)).abs() > 1e-6 || (frame.height - f64::from(MASTER_SIZE)).abs() > 1e-6 {
        return Err("icon frame must be 1024 × 1024".into());
    }
    let xf = doc.world_transform(frame.id).as_coeffs();
    if (xf[0] - 1.0).abs() > 1e-6 || xf[1].abs() > 1e-6 || xf[2].abs() > 1e-6 || (xf[3] - 1.0).abs() > 1e-6 {
        return Err("icon frame must be unrotated and unscaled".into());
    }
    // The owner-designed outer frame already contains the 824 px tile and its 100 px margins.
    let view = View { origin: (xf[4], xf[5]), scale: 1.0, width: MASTER_SIZE, height: MASTER_SIZE };
    let mut pixmap = Pixmap::new(MASTER_SIZE, MASTER_SIZE).ok_or("icon canvas could not be created")?;
    lizard_render::render_nodes_into(&doc, &[frame.id], &view, &mut pixmap);
    lizard_render::encode_png(&pixmap)
}

fn resized_png(master: &image::DynamicImage, size: u32) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    master.resize_exact(size, size, FilterType::Lanczos3).write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn generate_inner(root: &Path) -> Result<(), String> {
    let dir = root.join("assets/app-icon");
    let master_png = icon_png(root)?;
    let master = image::load_from_memory(&master_png).map_err(|e| e.to_string())?;
    let mut pngs = std::collections::BTreeMap::new();
    for size in [16u32, 20, 24, 32, 40, 48, 64, 128, 256, 512, 1024] {
        let png = if size == MASTER_SIZE { master_png.clone() } else { resized_png(&master, size)? };
        pngs.insert(size, png);
    }
    for size in [16u32, 32, 48, 64, 128, 256, 512, 1024] {
        let png = pngs.get(&size).ok_or("icon size missing")?;
        std::fs::write(dir.join(format!("lizard-{size}.png")), png).map_err(|e| e.to_string())?;
        if size <= 512 {
            let hicolor = dir.join(format!("hicolor/{size}x{size}/apps"));
            std::fs::create_dir_all(&hicolor).map_err(|e| e.to_string())?;
            std::fs::write(hicolor.join("com.brava.lizard.png"), png).map_err(|e| e.to_string())?;
        }
    }
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let mut ico = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut offset = 6 + 16 * sizes.len() as u32;
    for size in sizes {
        let png = pngs.get(&size).ok_or("ICO size missing")?;
        let entry_size = if size >= 256 { 0 } else { size as u8 };
        ico.extend_from_slice(&[entry_size, entry_size, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for size in sizes {
        ico.extend_from_slice(pngs.get(&size).ok_or("ICO size missing")?);
    }
    std::fs::write(dir.join("lizard.ico"), ico).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn generate(root: &Path) -> bool {
    match generate_inner(root) {
        Ok(()) => {
            eprintln!("icon: rendered {SOURCE} into assets/app-icon");
            true
        }
        Err(error) => {
            eprintln!("icon: {error}");
            false
        }
    }
}
