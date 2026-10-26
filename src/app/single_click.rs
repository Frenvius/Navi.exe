use super::*;

impl App {
    /// TrackMouseEvent's 4x4 hover rectangle: movement beyond it restarts the wait.
    pub fn row_hover(self: &Rc<Self>, row: usize, over: bool, x: f32, y: f32) {
        if !over {
            if self.hover_anchor.get().is_some_and(|a| a.0 == row) {
                self.hover_select_timer.stop();
                self.hover_anchor.set(None);
            }
            return;
        }
        if self.hover_anchor.get().is_some_and(|a| a.0 == row && (a.1-x).abs() <= 2.0 && (a.2-y).abs() <= 2.0) { return; }
        self.hover_select_timer.stop();
        self.hover_anchor.set(Some((row,x,y)));
        let (path, request, delay) = {
            let st = self.state.borrow();
            if !st.click_settings.single_click || st.renaming.is_some() { return; }
            let Some(path) = Self::row_path(&st,row) else { return };
            if st.selected.contains(&path) { return; }
            (path,st.request,st.click_settings.hover_ms)
        };
        let weak=Rc::downgrade(self);
        self.hover_select_timer.start(slint::TimerMode::SingleShot,std::time::Duration::from_millis(delay as u64),move||{
            let Some(app)=weak.upgrade() else { return };
            let ui=app.ui();
            let shell=ui.global::<Shell>();
            if !shell.get_active() || shell.get_menu_open() || app.hover_anchor.get().map(|a|a.0) != Some(row) { return; }
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState,VK_CONTROL,VK_SHIFT,VK_LBUTTON,VK_RBUTTON};
                if [VK_CONTROL,VK_SHIFT,VK_LBUTTON,VK_RBUTTON].iter().any(|key|GetAsyncKeyState(key.0 as i32)<0){return;}
            }
            {
                let st=app.state.borrow();
                if st.request!=request || !st.click_settings.single_click || st.renaming.is_some() || Self::row_path(&st,row).as_ref()!=Some(&path) { return; }
            }
            // Native hover selection also takes the keyboard focus from the search box.
            app.ui().invoke_focus_list();
            app.set_selection(HashSet::from([path.clone()]),Some(path.clone()),Some(path.clone()));
        });
    }
}
