use windows::Win32::Foundation::HWND;

pub fn context(hwnd: HWND, point: (f32, f32)) -> bool {
    let inside = super::WINDOW.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|window| window.scroll.view)
            .is_some_and(|view| {
                point.0 >= view.x
                    && point.0 < view.x + view.right
                    && point.1 >= view.y
                    && point.1 < view.y + view.bottom
            })
    });
    if !inside {
        return false;
    }
    let group = super::WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        window.list.interaction.pointer = Some(point);
        window.list.interaction.header_at(
            window.list.folder.as_ref()?,
            window.scroll.view?,
            &window.scroll,
        )
    });
    if let Some(row) = group {
        super::menu_actions::group(hwnd, row, point);
        return true;
    }
    super::WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.interaction.selection.selected.clear();
            window.list.interaction.selection.anchor = None;
            window.list.interaction.mark_all();
        }
    });
    super::input::flush(hwnd);
    super::menu_actions::background(hwnd, point, super::menu::Origin::Pointer);
    true
}
