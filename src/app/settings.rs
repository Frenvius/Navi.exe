//! App-owned virtual location; session journals keep its ID like any other tab.
use super::*;
impl App {
    pub fn open_settings(self: &Rc<Self>) {
        let existing = {
            let s = self.state.borrow();
            s.tabs
                .iter()
                .position(|t| t.history.iter().any(|p| ids::is_settings(p)))
        };
        if let Some(index) = existing {
            let history = {
                let s = self.state.borrow();
                s.tabs[index]
                    .history
                    .iter()
                    .position(|p| ids::is_settings(p))
                    .unwrap()
            };
            self.select_tab(index);
            self.go_to_history(history);
            self.refresh_settings();
        } else {
            self.new_tab(ids::SETTINGS);
        }
    }
    pub fn refresh_settings(&self) {
        if !ids::is_settings(&self.current_path()) {
            return;
        }
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        match super::default_manager::status() {
            Ok(status) => {
                shell.set_default_enabled(status.enabled);
                shell.set_default_recovery(status.recovery);
                if (status.enabled || !status.recovery)
                    && shell.get_default_error().as_str() == tr("settings.default.changed")
                {
                    shell.set_default_error(SharedString::new());
                }
                if status.recovery && !status.enabled && shell.get_default_error().is_empty() {
                    shell.set_default_error(tr("settings.default.changed").into());
                }
            }
            Err(e) => shell.set_default_error(
                crate::i18n::trf("settings.default.error", &[&e.to_string()]).into(),
            ),
        }
    }
    pub fn set_default_manager(&self, enabled: bool) {
        let result = super::default_manager::set_enabled(enabled);
        self.ui().global::<Shell>().set_default_error(match result {
            Ok(()) => SharedString::new(),
            Err(e) => crate::i18n::trf("settings.default.error", &[&e.to_string()]).into(),
        });
        self.refresh_settings();
    }
    pub fn list_settings(self: &Rc<Self>) {
        {
            let mut s = self.state.borrow_mut();
            s.loaded = true;
            s.crumbs = vec![Crumb {
                nav_root: None,
                id: ids::SETTINGS.into(),
                name: tr("settings.title").into(),
                icon: 0,
                has_subfolders: false,
            }];
            let at = s.current;
            s.tabs[at].title = tr("settings.title").into();
            s.tabs[at].icon = 0;
        }
        self.ui().global::<Shell>().set_search_busy(false);
        self.ui().global::<Shell>().set_search_indicator(false);
        self.sync_all();
        self.refresh_settings();
    }
}
