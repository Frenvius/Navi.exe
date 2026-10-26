//! Read preferences without rewriting Explorer's settings. Several View-page values
//! do not broadcast ShellState, so watch the persisted values as well as that message.
use super::*;

fn privacy() -> Vec<Option<Vec<u8>>> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer")
        .ok();
    [
        "ShowRecent",
        "ShowFrequent",
        "ShowRecommendations",
        "ShowCloudFilesInQuickAccess",
        "ShowDriveLettersFirst",
    ]
    .iter()
    .map(|name| {
        key.as_ref()
            .and_then(|k| k.get_raw_value(name).ok())
            .map(|v| v.bytes)
    })
    .collect()
}
impl App {
    pub(super) fn watch_preferences(self: &Rc<Self>) {
        let mut last = listing::settings();
        let mut last_privacy = privacy();
        let weak = Rc::downgrade(self);
        self.preferences_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                let Some(app) = weak.upgrade() else { return };
                let next = listing::settings();
                let next_privacy = privacy();
                if next == last && next_privacy == last_privacy {
                    return;
                }
                if next.icons_only != last.icons_only || next.type_overlay != last.type_overlay {
                    app.images.borrow_mut().clear_thumbs();
                }
                last = next;
                last_privacy = next_privacy;
                app.hide_key_tip();
                app.hide_tip();
                // Keep hidden status controls out of keyboard traversal immediately.
                let ui = app.ui();
                let shell = ui.global::<Shell>();
                if !next.status_bar && shell.get_focus_zone() == "status" {
                    ui.invoke_focus_list();
                }
                app.reload();
                app.refresh_nav();
            },
        );
    }
}
