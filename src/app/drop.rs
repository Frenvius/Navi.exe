//! Drag and drop glue: which folder a point over the window drops into, and drag starts.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use windows::Win32::Foundation::POINT;

use super::{ids, with_app, App, ROW_LEFT};
use crate::dnd::{Resolver, Spot, Target};
use windows::Win32::System::Ole::DROPEFFECT_MOVE;
use crate::Shell;

const CONTENT_TOP: f32 = 136.0;

/// Where the panes start: under the bars, or at the top in full screen.
fn content_top() -> f32 {
    if crate::frame::is_full_screen() { 0.0 } else { CONTENT_TOP }
}
const STATUS_HEIGHT: f32 = 24.0;
const HEADER_HEIGHT: f32 = 25.0;
/// The tree opens a folder held under a drag this long (the double-click time).
const EXPAND_DELAY: Duration = Duration::from_millis(500);
/// Near the pane's top or bottom edge a drag scrolls it a row at a time, this often.
const SCROLL_EDGE: f32 = 16.0;
const SCROLL_EVERY: Duration = Duration::from_millis(100);
const TAB_DELAY: Duration = Duration::from_millis(200);
const TAB_TIMER: usize = 0x5441_4246;

unsafe extern "system" fn tab_timer(hwnd: windows::Win32::Foundation::HWND, _: u32, id: usize, _: u32) {
    let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), id);
    with_app(|app| app.activate_file_tab());
}

/// Reported by the rendered strip, so compressed tabs use exactly its integer pitch.
#[derive(Clone, Copy, Default)]
pub(super) struct TabDropLayout {
    pub left: f32,
    pub top: f32,
    pub pitch: f32,
}

impl TabDropLayout {
    fn hit(self, x: f32, y: f32, count: usize) -> Option<usize> {
        if self.pitch <= 0.0 || x < self.left || y < self.top || y >= self.top + 32.0 {
            return None;
        }
        let index = ((x - self.left) / self.pitch).floor() as usize;
        (index < count).then_some(index)
    }
}

pub struct DropResolver;

impl Resolver for DropResolver {
    fn target_at(&self, client: POINT) -> Option<Target> {
        let mut out = None;
        with_app(|app| {
            app.hover_file_tab(app.file_tab_at(client));
            out = app.drop_target_at(client);
            app.drag_tick(client, out.as_ref().and_then(|t| t.spot));
        });
        out
    }

    fn highlight(&self, spot: Option<Spot>) {
        with_app(|app| {
            let ui = app.ui();
            let shell = ui.global::<Shell>();
            shell.set_drop_row(match spot { Some(Spot::Row(r)) => r as i32, _ => -1 });
            shell.set_nav_drop_row(match spot { Some(Spot::Nav(r))|Some(Spot::Pin{row:Some(r),..}) => r as i32, _ => -1 });
            shell.set_nav_insert_y(match spot {Some(Spot::Pin{line,..})=>line as f32,_=>-1.0});
            shell.set_crumb_drop(match spot { Some(Spot::Crumb(r)) => r as i32, _ => -1 });
            if spot.is_none() {
                app.state.borrow_mut().drag_hold = None;
            }
        });
    }

    fn caption(&self, caption: Option<(String, windows::Win32::System::Ole::DROPEFFECT, POINT)>) {
        use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK};
        with_app(|app| {
            let ui = app.ui();
            let shell = ui.global::<Shell>();
            let scale = ui.window().scale_factor();
            // The header rejects drops, showing its compact forbidden badge instead
            // of a Move/Copy description. The preview can extend above this window.
            if let Some((_, effect, at)) = &caption {
                if *effect == windows::Win32::System::Ole::DROPEFFECT_NONE {
                    shell.set_drag_caption(Default::default());
                    app.show_tab_drop_preview(*at);
                    return;
                }
            }
            app.hide_tab_drop_preview();
            match caption {
                Some((text, effect, at)) => {
                    // The XAML drag UI's glyphs: copy +, link a chain, move an arrow.
                    let glyph = if effect.0 & DROPEFFECT_MOVE.0 != 0 {
                        "\u{E8AD}"
                    } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
                        "\u{E710}"
                    } else if effect.0 & DROPEFFECT_LINK.0 != 0 {
                        "\u{E71B}"
                    } else {
                        ""
                    };
                    shell.set_drag_caption(text.into());
                    shell.set_drag_glyph(glyph.into());
                    shell.set_drag_x(at.x as f32 / scale);
                    shell.set_drag_y(at.y as f32 / scale);
                }
                None => shell.set_drag_caption(Default::default()),
            }
        });
    }

    fn entered(&self, files: Vec<String>) {
        with_app(|app| {
            app.cancel_file_tab_hover();
            app.drag_folders.set(!files.is_empty() && files.iter().all(|p|std::path::Path::new(p).is_dir() || crate::shell::ops::is_folder(p)));
            app.ui().global::<Shell>().set_file_drag(!files.is_empty());
            app.ui().global::<Shell>().set_drag_image(Default::default());
            let Some(first) = files.into_iter().next() else { return };
            std::thread::spawn(move || {
                let Ok(picture) = crate::shell::icons::tab_drag_picture(&first, 80) else { return };
                super::post(move |app| {
                    let picture = super::images::to_image(&picture);
                    app.ui().global::<Shell>().set_drag_image(picture.clone());
                    if let Some(preview) = app.drop_preview.borrow().as_ref() {
                        preview.set_picture(picture);
                    }
                });
            });
        });
    }

    fn tab_preview(&self, client: POINT) -> bool {
        let mut tab = false;
        with_app(|app| tab = app.file_tab_at(client).is_some());
        tab
    }

    fn left(&self) {
        with_app(|app| {
            // Explorer's already-started 200ms activation survives a quick release
            // and even leaving the header. Only the drag feedback is cleared here.
            app.tab_drop_hover.set(None);
            app.drag_folders.set(false);
            let ui = app.ui();
            ui.global::<Shell>().set_file_drag(false);
            ui.global::<Shell>().set_tab_drop_gap(false);
        });
    }

    fn dropped(&self, transfer: Option<crate::shell::ops::Transfer>) {
        with_app(|app| {
            let ui = app.ui();
            ui.global::<Shell>().set_file_drag(false);
            ui.global::<Shell>().set_tab_drop_gap(false);
            if let Some(t) = transfer {
                super::undo::record_transfer(app, t);
            }
            app.reload();
            app.refresh_nav();
        });
    }
}

impl App {
    fn file_tab_at(&self, client: POINT) -> Option<usize> {
        let ui = self.ui();
        if !ui.global::<Shell>().get_file_drag() {
            return None;
        }
        self.tab_at_client(client)
    }

    fn tab_at_client(&self, client: POINT) -> Option<usize> {
        if crate::frame::is_full_screen() { return None; }
        let ui = self.ui();
        let scale = ui.window().scale_factor();
        self.tab_drop_layout.get().hit(client.x as f32 / scale, client.y as f32 / scale, self.state.borrow().tabs.len())
    }

    fn cursor_tab(&self) -> Option<usize> {
        let mut cursor = POINT::default();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut cursor).ok()?;
            if !windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd.get(), &mut cursor).as_bool() { return None; }
        }
        self.tab_at_client(cursor)
    }

    fn cancel_file_tab_hover(&self) {
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd.get()), TAB_TIMER) };
        self.tab_drop_pending.set(None);
        self.tab_drop_hover.set(None);
        self.ui().global::<Shell>().set_tab_drop_gap(false);
    }

    fn hover_file_tab(self: &Rc<Self>, index: Option<usize>) {
        if self.tab_drop_hover.get() == index { return; }
        self.ui().global::<Shell>().set_tab_drop_gap(false);
        self.tab_drop_hover.set(index);
        let Some(index) = index else { return };
        self.tab_drop_pending.set(Some((index, Instant::now())));
        unsafe { windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd.get()), TAB_TIMER, TAB_DELAY.as_millis() as u32, Some(tab_timer)); }
    }

    fn activate_file_tab(self: &Rc<Self>) {
        let Some((index, started)) = self.tab_drop_pending.take() else { return };
        if index >= self.state.borrow().tabs.len() { return; }
        if std::env::var_os("DEBUG_TIMING").is_some() { eprintln!("tab activation {}ms", started.elapsed().as_millis()); }
        self.ui().global::<Shell>().set_tab_drop_gap(self.ui().global::<Shell>().get_file_drag() && self.cursor_tab().is_some());
        self.select_tab(index);
        self.ui().window().request_redraw();
        // OLE owns a nested message loop during SHDoDragDrop. Paint the requested frame
        // there instead of waiting for the next outer Slint event-loop iteration.
        unsafe {
            use windows::Win32::Graphics::Gdi::{RedrawWindow, RDW_INVALIDATE, RDW_UPDATENOW};
            let _ = RedrawWindow(Some(self.hwnd.get()), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
        }
    }

    fn show_tab_drop_preview(&self, at: POINT) {
        use slint::PhysicalPosition;
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        if self.drop_preview.borrow().is_none() {
            let made = crate::frame::creating_popup(self.hwnd.get(), || crate::DropPreview::new().expect("drag preview"));
            *self.drop_preview.borrow_mut() = Some(made);
        }
        let preview = self.drop_preview.borrow().as_ref().unwrap().clone_strong();
        let ui = self.ui();
        preview.global::<crate::Theme>().set_dark(ui.global::<crate::Theme>().get_dark());
        preview.set_picture(ui.global::<Shell>().get_drag_image());
        let mut screen = at;
        let _ = unsafe { ClientToScreen(self.hwnd.get(), &mut screen) };
        let scale = ui.window().scale_factor();
        preview.window().set_position(PhysicalPosition::new(
            screen.x - (40.0 * scale).round() as i32, screen.y - (70.0 * scale).round() as i32));
        let _ = preview.show();
        Self::style_tab_drop_preview(preview.as_weak(), 0);
    }

    fn style_tab_drop_preview(preview: slint::Weak<crate::DropPreview>, attempt: u32) {
        let Some(window) = preview.upgrade() else { return };
        let hwnd = crate::hwnd_of(window.window());
        if hwnd.is_invalid() {
            if attempt < 30 {
                slint::Timer::single_shot(Duration::from_millis(1), move || Self::style_tab_drop_preview(preview, attempt + 1));
            }
        } else {
            crate::frame::style_drop_preview(hwnd);
        }
    }

    fn hide_tab_drop_preview(&self) {
        let preview = self.drop_preview.borrow().as_ref().map(|p| p.clone_strong());
        if let Some(preview) = preview { let _ = preview.hide(); }
    }

    fn drop_target_at(&self, client: POINT) -> Option<Target> {
        let ui = self.ui();
        let shell = ui.global::<Shell>();
        let scale = ui.window().scale_factor();
        let (x, y) = (client.x as f32 / scale, client.y as f32 / scale);
        let size = ui.window().size().to_logical(scale);
        if let Some(target) = self.crumb_target_at(x, y) {
            return Some(target);
        }
        let st = self.state.borrow();
        let pane = if st.nav_pane { shell.get_sidebar_width() } else { 0.0 };
        let left = if st.nav_pane { pane + 6.0 } else { 0.0 };
        if y < content_top() || y > size.height - STATUS_HEIGHT {
            return None;
        }
        if x < left {
            drop(st);
            return self.nav_target_at(x - 2.0, y - content_top(), pane);
        }
        let folder = st.path.clone();
        let (vx, vy) = (x - left, y - content_top());
        let row = if Self::drives_view(&st) {
            let columns = st.columns.max(1);
            let (cx, cy) = (vx - 14.0, vy - 34.0);
            if cx < 0.0 || cy < 0.0 || (cx % 258.0) > 250.0 {
                None
            } else {
                Some((cy / 52.0) as usize * columns + (cx / 258.0) as usize)
            }
        } else if Self::icons_spec(&st).is_some() {
            let (cx, cy) = (vx, vy - shell.get_list_scroll_y());
            st.cells.iter().position(|c| cx >= c[0] && cx < c[0] + c[2] && cy >= c[1] && cy < c[1] + c[3])
        } else {
            let total = shell.get_col_name() + shell.get_columns_width();
            let cy = vy - HEADER_HEIGHT - shell.get_list_scroll_y();
            let cx = vx - shell.get_list_scroll_x();
            (cx >= ROW_LEFT && cx < ROW_LEFT + total - 18.0).then(|| App::row_at(&st, cy)).flatten()
        };
        // Only folders (and drives, the Recycle Bin...) take drops onto an item.
        let item = row
            .and_then(|r| st.order.get(r).map(|&i| (r, &st.entries[i])))
            .filter(|(_, e)| e.is_dir || !e.is_fs);
        match item {
            Some((r, e)) => Some(Target { folder, item: Some(e.path.clone()), spot: Some(Spot::Row(r)) }),
            None if ids::is_virtual(&folder) || folder.is_empty() => None,
            None => Some(Target { folder, item: None, spot: None }),
        }
    }

    /// A crumb of the address bar takes a drop into its location.
    fn crumb_target_at(&self, x: f32, y: f32) -> Option<Target> {
        let st = self.state.borrow();
        let hidden = self.ui().global::<Shell>().get_hidden_crumbs().max(0) as usize;
        (hidden..st.crumbs.len()).find_map(|i| {
            let (left, top) = *st.crumb_spots.get(i)?;
            let width = *st.crumb_widths.get(i)?;
            (x >= left && x < left + width && y >= top && y < top + 24.0).then(|| Target {
                folder: st.crumbs[i].id.clone(),
                item: Some(st.crumbs[i].id.clone()),
                spot: Some(Spot::Crumb(i)),
            })
        })
    }

    /// A row of the navigation pane (at a point in the pane) takes a drop into its item.
    fn nav_target_at(&self, x: f32, y: f32, pane: f32) -> Option<Target> {
        if x<0.0 || x>=pane-if self.nav_extent().1>0 {17.0}else{0.0}{return None;}
        if self.drag_folders.get() {
            let st=self.state.borrow();let ui=self.ui();let shell=ui.global::<Shell>();
            let pins=st.nav.rows.iter().enumerate().filter_map(|(i,r)|match &r.reference {
                super::navtree::RowRef::Node{section,path} if *section==super::navtree::PINS && path.len()==1=>Some(i),_=>None
            }).collect::<Vec<_>>();
            let make=|at,line,row|Some(Target{folder:ids::QUICK_ACCESS.into(),item:None,spot:Some(Spot::Pin{at,line,row})});
            if let Some(row)=self.nav_row_at(x,y,pane) {
                if st.nav.node(&st.nav.rows[row].reference).is_some_and(|n|ids::same(&n.node.id,ids::HOME)) {return make(usize::MAX,-1,Some(row));}
                if let Some(at)=pins.iter().position(|&r|r==row) {
                    let top=4.0+row as f32*24.0+shell.get_nav_scroll_y();
                    if y-top<8.0{return make(at,top as i32,None);}
                    if y-top>16.0{return make(at+1,(top+24.0) as i32,None);}
                }
            } else if let (Some(&first),Some(&last))=(pins.first(),pins.last()) {
                let top=4.0+first as f32*24.0+shell.get_nav_scroll_y();let bottom=4.0+(last+1) as f32*24.0+shell.get_nav_scroll_y();
                if y>=top-24.0 && y<top{return make(0,top as i32,None);}
                if y>=bottom && y<bottom+24.0{return make(pins.len(),bottom as i32,None);}
            }
        }
        let row = self.nav_row_at(x, y, pane)?;
        let st = self.state.borrow();
        let node = st.nav.node(&st.nav.rows.get(row)?.reference)?;
        Some(Target { folder: node.node.id.clone(), item: Some(node.node.id.clone()), spot: Some(Spot::Nav(row)) })
    }

    /// While a drag is over the window: a tree row held under it opens, and near the pane's
    /// edges the tree scrolls.
    fn drag_tick(self: &Rc<Self>, client: POINT, spot: Option<Spot>) {
        if !matches!(spot,Some(Spot::Nav(_)|Spot::Pin{..})) {
            self.state.borrow_mut().drag_hold = None;
            return;
        }
        let ui = self.ui();
        let y = client.y as f32 / ui.window().scale_factor() - content_top();
        let page = ui.global::<Shell>().get_nav_page();
        let now = Instant::now();
        let edge = if y < SCROLL_EDGE { -1 } else if y > page - SCROLL_EDGE { 1 } else { 0 };
        let due = self.state.borrow().drag_scrolled.map(|t| now - t >= SCROLL_EVERY).unwrap_or(true);
        if edge != 0 && due {
            self.state.borrow_mut().drag_scrolled = Some(now);
            self.nav_scroll_by(edge);
        }
        let Some(Spot::Nav(row))=spot else {
            self.state.borrow_mut().drag_hold=None;
            return;
        };
        let expand = {
            let mut st = self.state.borrow_mut();
            let Some(reference) = st.nav.rows.get(row).map(|r| r.reference.clone()) else { return };
            match &st.drag_hold {
                Some((held, since)) if *held == reference => {
                    now - *since >= EXPAND_DELAY && st.nav.node(&reference).map(|n| n.node.has_children && !n.expanded).unwrap_or(false)
                }
                _ => {
                    st.drag_hold = Some((reference, now));
                    false
                }
            }
        };
        if expand {
            self.state.borrow_mut().drag_hold = None;
            self.expand_nav_quietly(row);
        }
    }

    /// Starts a shell drag of the selection (Explorer's own data object and drag image).
    pub fn start_drag(self: &Rc<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        crate::dnd::drag_apart(self.hwnd.get(), paths, |effect| super::post(move |app| app.drag_ended(effect)));
    }

    pub fn start_nav_drag(self:&Rc<Self>,row:usize){
        let Some((node,_))=self.nav_node(row) else{return};
        if !node.is_fs && !node.is_pinned{return;}
        crate::dnd::drag_apart(self.hwnd.get(),vec![node.id],|effect|super::post(move |app|app.drag_ended(effect)));
    }

    fn drag_ended(self: &Rc<Self>, effect: windows::Win32::System::Ole::DROPEFFECT) {
        // The OLE drag loop swallowed the button release; let the UI see one.
        let ui = self.ui();
        ui.window().dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(-100.0, -100.0),
            button: slint::platform::PointerEventButton::Left,
        });
        // OLE also consumed the move away from the source row. Clear that hover
        // before replaying the real pointer position, including a cancelled drag.
        ui.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
        self.rehover();
        self.state.borrow_mut().pending_narrow = None;
        if effect != windows::Win32::System::Ole::DROPEFFECT_NONE {
            self.reload();
        }
    }
}
