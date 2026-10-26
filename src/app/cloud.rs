//! Cloud flyout presentation; provider COM lives in shell::cloud's worker apartment.
use super::*;
use crate::shell::cloud::{self, Request, Snapshot};
use crate::{CloudCommand, CloudData};
fn image(uri: &str) -> slint::Image {
    // Never download a provider URI just to paint its icon.
    let Ok(uri) = windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from(uri)) else {
        return Default::default();
    };
    if !uri.SchemeName().is_ok_and(|s| s == "file") {
        return Default::default();
    }
    let Ok(path) = uri.Path() else {
        return Default::default();
    };
    let path = windows::Foundation::Uri::UnescapeComponent(&path)
        .unwrap_or(path)
        .to_string()
        .trim_start_matches('/')
        .to_string();
    slint::Image::load_from_path(std::path::Path::new(&path)).unwrap_or_default()
}
impl App {
    pub(super) fn watch_cloud(self: &Rc<Self>) {
        let tx = cloud::start(|snapshot| super::post(move |app| app.update_cloud(snapshot)));
        *self.cloud_worker.borrow_mut() = Some(tx.clone());
        let weak = Rc::downgrade(self);
        let mut previous = String::new();
        self.cloud_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(100),
            move || {
                let Some(app) = weak.upgrade() else { return };
                let path = app.current_path();
                if path != previous {
                    previous = path.clone();
                    app.ui().global::<Shell>().set_cloud(Default::default());
                    let _ = tx.send(Request::Path(path));
                }
            },
        );
    }
    fn update_cloud(&self, snapshot: Snapshot) {
        if snapshot.path != self.current_path() {
            return;
        }
        let data = CloudData {
            label: snapshot.label.clone().into(),
            icon: image(&snapshot.icon),
            account: snapshot.account.clone().into(),
            quota: snapshot.quota.clone().into(),
            fraction: if snapshot.total == 0 {
                0.0
            } else {
                snapshot.used as f32 / snapshot.total as f32
            },
            color: slint::Color::from_argb_u8(
                snapshot.color[0],
                snapshot.color[1],
                snapshot.color[2],
                snapshot.color[3],
            ),
            more: snapshot.more.clone().into(),
            commands: ModelRc::new(VecModel::from(
                snapshot
                    .commands
                    .iter()
                    .enumerate()
                    .map(|(slot, c)| CloudCommand {
                        slot: slot as i32,
                        label: c.label.clone().into(),
                        description: c.description.clone().into(),
                        icon: image(&c.icon),
                        state: c.state,
                    })
                    .collect::<Vec<_>>(),
            )),
        };
        self.ui().global::<Shell>().set_cloud(data.clone());
        self.update_cloud_popup(data);
        *self.cloud_snapshot.borrow_mut() = snapshot;
    }
    pub(super) fn cloud_open(self: &Rc<Self>, x: f32, y: f32, keyboard: bool) {
        if self.ui().global::<Shell>().get_cloud().label.is_empty() {
            return;
        }
        let open = self
            .menu
            .borrow()
            .as_ref()
            .is_some_and(|m| m.owner == "cloud-status");
        menus::dismiss(self);
        if open {
            return;
        }
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        shell.set_menu_open(true);
        shell.set_menu_owner("cloud-status".into());
        *self.menu.borrow_mut() = Some(menus::OpenMenu {
            owner: "cloud-status".into(),
            items: Vec::new(),
            buttons: Vec::new(),
            sub_parent: None,
        });
        self.show_menu_popup(
            crate::PopupData {
                cloud: true,
                keyboard,
                ..Default::default()
            },
            x,
            y,
            x,
            0.0,
            crate::frame::flyout_border(true),
            None,
        );
    }
    pub(super) fn cloud_command(self: &Rc<Self>, slot: i32) {
        let snapshot = self.cloud_snapshot.borrow();
        let revision = snapshot.revision;
        let valid = snapshot.path == self.current_path()
            && slot >= 0
            && snapshot
                .commands
                .get(slot as usize)
                .is_some_and(|c| c.state == 0);
        drop(snapshot);
        if !valid {
            return;
        }
        menus::dismiss(self);
        if let Some(tx) = self.cloud_worker.borrow().as_ref() {
            let _ = tx.send(Request::Invoke(revision, slot as usize));
        }
    }
}
