//! Desktop update notice and controls. Downloading and installation belong to platform services.

use egui::{Align2, Context, Id, RichText, vec2};

use crate::i18n::tr;
use crate::primitives::{Button, ButtonVariant};
use crate::theme::{Tokens, semibold};
use crate::{LizardApp, UpdateStatus};

fn begin_update(app: &mut LizardApp, ctx: &Context) {
    ctx.data_mut(|data| data.remove_temp::<String>(Id::new("lizard_update_dismissed")));
    app.services.update_prepare();
}

pub fn show(app: &mut LizardApp, ctx: &Context) {
    if app.windows.current != 0 {
        return;
    }
    let status = app.services.update_status();
    let dismissed = Id::new("lizard_update_dismissed");
    let ready_to_install = match &status {
        UpdateStatus::Ready(version) => ctx.data(|data| data.get_temp::<String>(dismissed).as_deref() != Some(version.as_str())),
        _ => false,
    };
    if ready_to_install && app.session.docs.iter().all(|document| !document.is_dirty()) && app.windows.list.is_empty() {
        match app.services.update_install() {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => app.toast(error),
        }
        return;
    }
    if matches!(&status, UpdateStatus::Checking | UpdateStatus::Preparing(_)) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    } else if matches!(&status, UpdateStatus::Current | UpdateStatus::Error(_)) {
        ctx.request_repaint_after(std::time::Duration::from_secs(60));
    }
    if app.ui.preferences {
        return;
    }
    let version = match &status {
        UpdateStatus::Available(version) | UpdateStatus::Preparing(version) | UpdateStatus::Ready(version) | UpdateStatus::Failed(version, _) => {
            version
        }
        _ => return,
    };
    if ctx.data(|data| data.get_temp::<String>(dismissed).as_deref() == Some(version.as_str())) {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let mut close = false;
    egui::Area::new(Id::new("lizard_update_notice")).anchor(Align2::RIGHT_TOP, vec2(-24.0, 64.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::NONE
            .fill(t.panel)
            .stroke(egui::Stroke::new(1.0, t.border))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .shadow(egui::Shadow { offset: [0, 8], blur: 20, spread: 0, color: t.shadow })
            .show(ui, |ui| {
                ui.set_width(300.0_f32.min(screen.width() - 48.0));
                ui.label(RichText::new(tr("Lizard update available")).font(semibold(14.0)).color(t.text));
                ui.add_space(5.0);
                ui.label(RichText::new(format!("{} {version}", tr("Version"))).color(t.text_secondary));
                ui.add_space(12.0);
                match &status {
                    UpdateStatus::Available(_) => {
                        ui.horizontal(|ui| {
                            if Button::new(tr("Update")).show(ui).clicked() {
                                begin_update(app, ctx);
                            }
                            if Button::new(tr("Later")).variant(ButtonVariant::Secondary).show(ui).clicked() {
                                close = true;
                            }
                        });
                    }
                    UpdateStatus::Preparing(_) => {
                        ui.spinner();
                        ui.label(tr("Downloading and verifying the update…"));
                    }
                    UpdateStatus::Failed(_, error) => {
                        ui.label(RichText::new(error).color(t.text_secondary));
                        ui.horizontal(|ui| {
                            if Button::new(tr("Update")).show(ui).clicked() {
                                begin_update(app, ctx);
                            }
                            if Button::new(tr("Later")).variant(ButtonVariant::Secondary).show(ui).clicked() {
                                close = true;
                            }
                        });
                    }
                    UpdateStatus::Ready(_) => {
                        let dirty = app.session.docs.iter().any(|document| document.is_dirty());
                        let extra_windows = !app.windows.list.is_empty();
                        if dirty {
                            ui.label(tr("Save your open files before updating."));
                        } else if extra_windows {
                            ui.label(tr("Close the other Lizard windows before updating."));
                        }
                        ui.horizontal(|ui| {
                            if Button::new(tr("Restart and update")).enabled(!dirty && !extra_windows).show(ui).clicked() {
                                match app.services.update_install() {
                                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                    Err(error) => app.toast(error),
                                }
                            }
                            if Button::new(tr("Later")).variant(ButtonVariant::Secondary).show(ui).clicked() {
                                close = true;
                            }
                        });
                    }
                    _ => {}
                }
            });
    });
    if close {
        ctx.data_mut(|data| data.insert_temp(dismissed, version.clone()));
    }
}

pub fn settings(app: &mut LizardApp, ui: &mut egui::Ui) {
    let status = app.services.update_status();
    if status == UpdateStatus::Unavailable {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(20.0);
    ui.label(RichText::new(tr("Updates")).font(semibold(13.0)));
    ui.add_space(8.0);
    let description = match &status {
        UpdateStatus::Checking => tr("Checking for updates…").to_owned(),
        UpdateStatus::Current => tr("Lizard is up to date.").to_owned(),
        UpdateStatus::Available(version) => format!("{} {version}", tr("New version available:")),
        UpdateStatus::Preparing(_) => tr("Downloading and verifying the update…").to_owned(),
        UpdateStatus::Ready(version) => format!("{} {version}", tr("Ready to install:")),
        UpdateStatus::Failed(_, error) => error.clone(),
        UpdateStatus::Installing => tr("Restarting to install the update…").to_owned(),
        UpdateStatus::Error(error) => error.clone(),
        UpdateStatus::Unavailable => String::new(),
    };
    ui.label(RichText::new(description).color(t.text_secondary));
    ui.add_space(10.0);
    if Button::new(tr("Check for updates"))
        .variant(ButtonVariant::Secondary)
        .enabled(!matches!(
            &status,
            UpdateStatus::Checking | UpdateStatus::Preparing(_) | UpdateStatus::Ready(_) | UpdateStatus::Failed(_, _) | UpdateStatus::Installing
        ))
        .show(ui)
        .clicked()
    {
        app.services.update_check();
    }
    match status {
        UpdateStatus::Available(_) => {
            ui.add_space(8.0);
            if Button::new(tr("Update")).show(ui).clicked() {
                begin_update(app, ui.ctx());
            }
        }
        UpdateStatus::Failed(_, _) => {
            ui.add_space(8.0);
            if Button::new(tr("Update")).show(ui).clicked() {
                begin_update(app, ui.ctx());
            }
        }
        UpdateStatus::Ready(_) => {
            let can_quit = app.session.docs.iter().all(|document| !document.is_dirty()) && app.windows.list.is_empty();
            ui.add_space(8.0);
            if Button::new(tr("Restart and update")).enabled(can_quit).show(ui).clicked() {
                match app.services.update_install() {
                    Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(error) => app.toast(error),
                }
            }
            if !can_quit {
                ui.label(tr("Save your open files and close other Lizard windows first."));
            }
        }
        _ => {}
    }
}
