//! Local, standalone native window for inspecting Lizard's production UI components.
//! This binary is a separate workspace package and is never included in the Lizard app bundle.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod catalog;

use figcraft_engine::Session;
use figcraft_ui_egui::{FigApp, NoServices, theme};

struct CatalogApp {
    fig: FigApp,
    state: catalog::State,
    fixture: catalog::Fixture,
}

impl eframe::App for CatalogApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        catalog::show(ui, &mut self.state, &mut self.fig, &self.fixture);
    }
}

fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--audit") {
        match catalog::audit() {
            Ok(count) => eprintln!("{count} catalog pages rendered with production components"),
            Err(error) => {
                eprintln!("Catalog audit failed: {error}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    figcraft_text::scan_system_fonts();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lizard · UI Components")
            .with_inner_size([1340.0, 880.0])
            .with_min_inner_size([900.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Lizard · UI Components",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx, true, 0);
            theme::apply(&cc.egui_ctx, theme::Tokens::dark());
            if std::env::args().any(|arg| arg == "--open-type-settings") {
                cc.egui_ctx.data_mut(|data| data.insert_temp(egui::Id::new("ty_settings_open").with("open"), true));
            }
            if std::env::args().any(|arg| arg == "--type-details") {
                cc.egui_ctx.data_mut(|data| data.insert_temp(egui::Id::new("ty_settings_tab"), 1usize));
            }
            let mut session = Session::new();
            let fixture = catalog::Fixture::new(&mut session);
            let mut state = catalog::State::default();
            if let Some(label) = std::env::args().skip_while(|arg| arg != "--page").nth(1) {
                let _ = state.select_page(&label);
            }
            Ok(Box::new(CatalogApp { fig: FigApp::new(session, Box::new(NoServices)), state, fixture }))
        }),
    )
}
