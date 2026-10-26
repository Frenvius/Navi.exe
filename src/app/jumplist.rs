use super::*;
impl App {
    pub(super) fn watch_jump_list(self: &Rc<Self>) {
        let tx = crate::shell::jumplist::start();
        let _ = tx.send(String::new());
        let weak = Rc::downgrade(self);
        let mut last = String::new();
        let mut privacy = crate::shell::jumplist::history_enabled();
        self.jump_timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                let Some(app) = weak.upgrade() else { return };
                let st = app.state.borrow();
                if !st.loaded || !st.error.is_empty() {
                    return;
                }
                let path = st.path.clone();
                drop(st);
                let next = crate::shell::jumplist::history_enabled();
                if path != last {
                    last = path.clone();
                    if !ids::is_virtual(&path) {
                        let _ = tx.send(path);
                    }
                }
                if privacy != next {
                    privacy = next;
                    let _ = tx.send(String::new());
                }
            },
        );
    }
}
