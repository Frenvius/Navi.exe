use std::time::{Duration, Instant};
use windows::core::{Error, Result};
use windows::Win32::Foundation::{E_FAIL, HANDLE, HWND};
use windows::Win32::UI::WindowsAndMessaging::SetPropW;

use super::{address::Target, chrome::Focus, WINDOW};

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;

const DEBOUNCE: usize = 70;
const ANIMATION: usize = 71;

#[derive(Default)]
pub struct State {
    pub options: crate::shell::search::Options,
    path: String,
    edited: Option<Instant>,
    started: Option<Instant>,
    first_hit: Option<Instant>,
    animation: bool,
    view: Option<(String, crate::app::view::ResolvedView)>,
}

pub enum Submission {
    Typing,
    Explicit,
}

pub fn edited(hwnd: HWND) {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.search.cancel();
            window.search.edited = Some(Instant::now());
        }
    });
    super::input::arm(hwnd, DEBOUNCE, 500);
    sync(hwnd);
}

pub fn submit(hwnd: HWND, submission: Submission) -> Result<()> {
    super::input::stop(hwnd, DEBOUNCE);
    let state = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        window.search.edited = None;
        let path = window.tabs.active().location.tab.path().to_owned();
        let query = window.address.search_edit.text.clone();
        let scope = crate::app::ids::search_parts(&path)
            .map_or(path.clone(), |(scope, _)| scope.to_owned());
        Some((
            path,
            query,
            scope,
            (
                window.address.search_edit.start,
                window.address.search_edit.end,
            ),
            window.editors.search,
        ))
    });
    let Some((path, query, scope, selection, control)) = state else {
        return Ok(());
    };
    if query.trim().is_empty() {
        close(hwnd)?;
        if matches!(submission, Submission::Typing) {
            super::chrome::set_focus(hwnd, Focus::Address(Target::Search));
        }
        return Ok(());
    }
    let id = crate::app::ids::search_id(&scope, query.trim());
    if path == id {
        if matches!(submission, Submission::Explicit) {
            super::chrome::set_focus(hwnd, Focus::List);
        }
        return Ok(());
    }
    let refine = crate::app::ids::is_search(&path);
    if refine {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                if let Some(folder) = window.list.folder.as_ref() {
                    let view = folder.view.clone();
                    window
                        .tabs
                        .active_mut()
                        .location
                        .views
                        .insert(id.clone(), view);
                }
            }
        });
    } else {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                if let Some((saved_scope, view)) = &window.search.view {
                    if crate::app::ids::same(saved_scope, &scope) {
                        let view = view.clone();
                        window
                            .tabs
                            .active_mut()
                            .location
                            .views
                            .insert(id.clone(), view);
                    }
                }
            }
        });
    }
    super::navigate(hwnd, super::Navigation::Open(id))?;
    if refine {
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                let location = &mut window.tabs.active_mut().location;
                let index = location.tab.index;
                if index > 0 {
                    location.tab.history.remove(index - 1);
                    location.tab.filters.remove(index - 1);
                    location.visits.remove(index - 1);
                    location.tab.index -= 1;
                    if !location.tab.history.contains(&path) {
                        location.views.remove(&path);
                    }
                }
            }
        });
    }
    sync(hwnd);
    if matches!(submission, Submission::Typing) {
        super::chrome::set_focus(hwnd, Focus::Address(Target::Search));
        super::edit::set_text(control, &query, selection)?;
        super::edit::notify_preview(hwnd, control);
    } else {
        super::chrome::set_focus(hwnd, Focus::List);
        WINDOW.with(|slot| {
            if let Some(window) = slot.borrow_mut().as_mut() {
                window.list.interaction.cues = true;
            }
        });
    }
    Ok(())
}

pub fn leaving(hwnd: HWND) {
    super::input::stop(hwnd, DEBOUNCE);
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.search.edited = None;
            if let Some((scope, _)) =
                crate::app::ids::search_parts(window.tabs.active().location.tab.path())
            {
                if let Some(folder) = &window.list.folder {
                    window.search.view = Some((scope.to_owned(), folder.view.clone()));
                }
            }
        }
    });
}

pub fn close(hwnd: HWND) -> Result<()> {
    super::input::stop(hwnd, DEBOUNCE);
    let target = WINDOW.with(|slot| {
        let slot = slot.borrow();
        let window = slot.as_ref()?;
        let tab = &window.tabs.active().location.tab;
        let (scope, _) = crate::app::ids::search_parts(tab.path())?;
        Some(
            tab.history[..tab.index]
                .iter()
                .rposition(|path| crate::app::ids::same(path, scope))
                .map_or_else(
                    || super::Navigation::Open(scope.to_owned()),
                    super::Navigation::History,
                ),
        )
    });
    if let Some(target) = target {
        super::navigate(hwnd, target)?;
    }
    sync(hwnd);
    Ok(())
}

pub fn cancel(hwnd: HWND) -> bool {
    super::input::stop(hwnd, DEBOUNCE);
    let cancelled = WINDOW.with(|slot| {
        slot.borrow_mut().as_mut().is_some_and(|window| {
            window.search.edited = None;
            window.list.cancel_search()
        })
    });
    sync(hwnd);
    cancelled
}

pub fn reload(hwnd: HWND) -> Result<()> {
    WINDOW.with(|slot| -> Result<()> {
        let mut slot = slot.borrow_mut();
        let Some(window) = slot.as_mut() else {
            return Ok(());
        };
        let path = window.tabs.active().location.tab.path().to_owned();
        let view = window
            .list
            .folder
            .as_ref()
            .map(|folder| folder.view.clone());
        window.list = super::list::List::search_start(
            &window.worker,
            hwnd,
            path,
            view,
            window.search.options,
        )
        .map_err(|error| Error::new(E_FAIL, error))?;
        window.search.path.clear();
        window.first_list = false;
        Ok(())
    })?;
    sync(hwnd);
    super::invalidate_all(hwnd);
    Ok(())
}

pub fn timer(hwnd: HWND, id: usize) -> bool {
    if id == DEBOUNCE {
        let remaining = WINDOW.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|window| window.search.edited)
                .and_then(|at| Duration::from_millis(500).checked_sub(at.elapsed()))
        });
        if let Some(remaining) = remaining {
            super::input::arm(hwnd, id, remaining.as_millis().max(1) as u32);
        } else if let Err(error) = submit(hwnd, Submission::Typing) {
            super::fail(error);
        }
        return true;
    }
    if id != ANIMATION {
        return false;
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.list.interaction.mark_all();
        }
    });
    sync(hwnd);
    super::invalidate_title(hwnd);
    super::invalidate_view(hwnd);
    true
}

pub fn sync(hwnd: HWND) {
    let state = WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        let path = window.tabs.active().location.tab.path().to_owned();
        let search = crate::app::ids::is_search(&path);
        let busy = window.list.search.busy();
        let changed = window.search.path != path;
        if changed {
            window.search.path = path.clone();
            window.search.started = busy.then(Instant::now);
            window.search.first_hit = None;
        }
        if search
            && window.search.first_hit.is_none()
            && window
                .list
                .folder
                .as_ref()
                .is_some_and(|folder| !folder.entries.is_empty())
        {
            window.search.first_hit = Some(Instant::now());
        }
        if busy && window.search.started.is_none() {
            window.search.started = Some(Instant::now());
        } else if !busy {
            window.search.started = None;
        }
        let elapsed = window.search.started.map(|at| at.elapsed());
        window.title.busy =
            if busy && elapsed.is_some_and(|elapsed| elapsed >= Duration::from_millis(1500)) {
                Some(elapsed.unwrap().as_secs_f32())
            } else {
                None
            };
        let pulse = window
            .search
            .first_hit
            .is_some_and(|at| at.elapsed() < Duration::from_millis(3550));
        let animate = busy || pulse;
        let transition = window.search.animation != animate;
        window.search.animation = animate;
        let query = changed.then(|| {
            crate::app::ids::search_parts(&path)
                .map_or("", |(_, query)| query)
                .to_owned()
        });
        Some((
            animate,
            transition,
            changed,
            query,
            window.editors.search,
            window.list.search.workers(),
            busy,
        ))
    });
    let Some((animate, transition, changed, query, control, workers, busy)) = state else {
        return;
    };
    if transition {
        if animate {
            super::input::arm(hwnd, ANIMATION, 16);
        } else {
            super::input::stop(hwnd, ANIMATION);
        }
    }
    if let Some(query) = query {
        let end = query.encode_utf16().count() as u32;
        if let Err(error) = super::edit::set_text(control, &query, (end, end)) {
            super::fail(error);
        }
        super::edit::notify_preview(hwnd, control);
    }
    unsafe {
        for (key, value) in [
            (windows::core::w!("NativeSearchWorkers"), workers),
            (windows::core::w!("NativeSearchBusy"), usize::from(busy)),
        ] {
            if let Err(error) = SetPropW(hwnd, key, Some(HANDLE(value as _))) {
                super::fail(error);
            }
        }
    }
    if changed {
        super::invalidate_title(hwnd);
    }
}

pub fn highlight(
    canvas: &super::render::Canvas,
    text: &mut super::text::TextEngine,
    theme: &super::theme::Theme,
    list: &super::list::List,
    value: &str,
    bounds: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    style: super::text::TextStyle,
) -> Result<()> {
    use windows::Win32::Graphics::Direct2D::Common::*;
    use windows::Win32::Graphics::Direct2D::*;
    let Some((_, query)) = list
        .folder
        .as_ref()
        .and_then(|folder| folder.crumbs.last())
        .and_then(|crumb| crate::app::ids::search_parts(&crumb.id))
    else {
        return Ok(());
    };
    let elapsed = list.search.hit_elapsed().as_secs_f32();
    let stage = ((elapsed - 2.032) / 1.5).clamp(0.0, 1.0);
    let interpolate = |from: D2D1_COLOR_F, to: D2D1_COLOR_F| D2D1_COLOR_F {
        r: from.r + (to.r - from.r) * stage,
        g: from.g + (to.g - from.g) * stage,
        b: from.b + (to.b - from.b) * stage,
        a: from.a + (to.a - from.a) * stage,
    };
    let name = style == super::text::GROUP_HEADER || style == super::text::SEARCH_NAME;
    if name && crate::app::search_query::name_range(value, query).is_none() {
        return Ok(());
    }
    for span in crate::app::search_query::literal_spans(value, query)
        .into_iter()
        .take(if name { 1 } else { usize::MAX })
    {
        let offset = text.layout(&value[..span.start], style, 4096.0)?.advance;
        let width = text.layout(&value[span], style, 4096.0)?.advance;
        if offset >= bounds.right - bounds.left {
            continue;
        }
        let plate = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: bounds.left + offset - 2.0,
                right: (bounds.left + offset + width + 2.0).min(bounds.right),
                ..bounds
            },
            radiusX: 3.0,
            radiusY: 3.0,
        };
        unsafe {
            canvas.brush.SetColor(&interpolate(
                if elapsed < 1.968 {
                    theme.hit_idle
                } else {
                    theme.hit_pulse
                },
                theme.hit,
            ));
            canvas.context.FillRoundedRectangle(&plate, &canvas.brush);
            canvas
                .brush
                .SetColor(&interpolate(theme.hit_idle_border, theme.hit_border));
            let inner = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: plate.rect.left + 0.5,
                    top: plate.rect.top + 0.5,
                    right: plate.rect.right - 0.5,
                    bottom: plate.rect.bottom - 0.5,
                },
                radiusX: 2.5,
                radiusY: 2.5,
            };
            canvas
                .context
                .DrawRoundedRectangle(&inner, &canvas.brush, 1.0, None);
            if stage == 1.0 {
                for (x, y) in [
                    (plate.rect.left + 2.0, plate.rect.top),
                    (plate.rect.right - 3.0, plate.rect.top),
                    (plate.rect.left, plate.rect.top + 2.0),
                    (plate.rect.right - 1.0, plate.rect.top + 2.0),
                    (plate.rect.left + 1.0, plate.rect.top + 1.0),
                    (plate.rect.right - 2.0, plate.rect.top + 1.0),
                    (plate.rect.left + 1.0, plate.rect.bottom - 2.0),
                    (plate.rect.right - 2.0, plate.rect.bottom - 2.0),
                    (plate.rect.left, plate.rect.bottom - 3.0),
                    (plate.rect.right - 1.0, plate.rect.bottom - 3.0),
                    (plate.rect.left + 2.0, plate.rect.bottom - 1.0),
                    (plate.rect.right - 3.0, plate.rect.bottom - 1.0),
                ] {
                    super::details::fill(
                        canvas,
                        D2D_RECT_F {
                            left: x,
                            top: y,
                            right: x + 1.0,
                            bottom: y + 1.0,
                        },
                        &theme.hit_corner,
                    );
                }
                if theme.content.r > 0.5 {
                    for (x, y) in [
                        (plate.rect.left + 1.0, plate.rect.top),
                        (plate.rect.right - 2.0, plate.rect.top),
                        (plate.rect.left, plate.rect.top + 1.0),
                        (plate.rect.right - 1.0, plate.rect.top + 1.0),
                        (plate.rect.left, plate.rect.bottom - 2.0),
                        (plate.rect.right - 1.0, plate.rect.bottom - 2.0),
                        (plate.rect.left + 1.0, plate.rect.bottom - 1.0),
                        (plate.rect.right - 2.0, plate.rect.bottom - 1.0),
                    ] {
                        super::details::fill(
                            canvas,
                            D2D_RECT_F {
                                left: x,
                                top: y,
                                right: x + 1.0,
                                bottom: y + 1.0,
                            },
                            &theme.hit_soft_corner,
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn draw_ring(
    canvas: &super::render::Canvas,
    theme: &super::theme::Theme,
    at: (f32, f32),
    phase: f32,
) -> Result<()> {
    use windows::Win32::Graphics::Direct2D::Common::*;
    use windows::Win32::Graphics::Direct2D::*;
    unsafe {
        let factory = canvas.context.GetFactory()?;
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        let sweep = (0.18 + 0.68 * ((phase * 2.0).sin() * 0.5 + 0.5)) * std::f32::consts::TAU;
        let mut point = D2D1_ELLIPSE::default().point;
        for step in 0..=48 {
            let angle = phase * 4.0 + sweep * step as f32 / 48.0;
            point.X = at.0 + 7.0 * angle.cos();
            point.Y = at.1 + 7.0 * angle.sin();
            if step == 0 {
                sink.BeginFigure(point, D2D1_FIGURE_BEGIN_HOLLOW);
            } else {
                sink.AddLine(point);
            }
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        let style = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                ..Default::default()
            },
            None,
        )?;
        canvas.brush.SetColor(&theme.busy_ring);
        canvas
            .context
            .DrawGeometry(&geometry, &canvas.brush, 1.5, &style);
    }
    Ok(())
}

pub fn refine(hwnd: HWND, keyword: &str, value: &str) -> Result<()> {
    let data = WINDOW.with(|slot| {
        slot.borrow().as_ref().map(|window| {
            (
                window.editors.search,
                crate::app::search_query::refine(&window.address.search_edit.text, keyword, value),
            )
        })
    });
    if let Some((control, query)) = data {
        let end = query.encode_utf16().count() as u32;
        super::edit::set_text(control, &query, (end, end))?;
        super::edit::notify_preview(hwnd, control);
        submit(hwnd, Submission::Explicit)?;
    }
    Ok(())
}

pub fn scope(hwnd: HWND, current_folder_only: bool) -> Result<()> {
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.search.options.current_folder_only = current_folder_only;
        }
    });
    reload(hwnd)
}

pub fn toggle(hwnd: HWND, which: u8) -> Result<()> {
    let Some(mut options) = options() else {
        return Ok(());
    };
    match which {
        0 => options.system_files = !options.system_files,
        1 => {
            options.zipped_folders = !options.zipped_folders;
            crate::shell::search::set_archived_files(options.zipped_folders)
                .map_err(|error| Error::new(E_FAIL, error))?;
        }
        2 => options.file_contents = !options.file_contents,
        _ => return Ok(()),
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow_mut().as_mut() {
            window.search.options = options;
        }
    });
    reload(hwnd)
}

pub fn options() -> Option<crate::shell::search::Options> {
    let archived = crate::shell::search::archived_files();
    WINDOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let window = slot.as_mut()?;
        window.search.options.zipped_folders = archived;
        Some(window.search.options)
    })
}
