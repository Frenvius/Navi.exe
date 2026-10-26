//! "Desfazer": what this window did itself (renames, new items, deletes to the Recycle Bin,
//! files moved or copied by pasting or dropping), newest last; and "Refazer" (Ctrl+Y) for
//! what it undid.

use std::rc::Rc;

use super::{App, AfterLoad};
use crate::shell::ops;

pub enum Undo {
    /// Items renamed from the first path to the second (several at once with F2 over a
    /// selection).
    Rename(Vec<(String, String)>),
    /// A new item; undoing it sends it to the Recycle Bin.
    Create(String),
    /// Items sent to the Recycle Bin; undoing it puts them back ("Desfazer Excluir").
    Delete(Vec<String>),
    /// Items moved from the first path to the second; undoing it moves them back.
    Move(Vec<(String, String)>),
    /// Items copied from the first path to the second; undoing it deletes the copies for
    /// good, without asking, as Explorer does.
    Copy(Vec<(String, String)>),
}

/// A paste or drop of files made these moves or copies.
pub fn record_transfer(app: &App, t: ops::Transfer) {
    if !t.moved.is_empty() {
        push(app, Undo::Move(t.moved));
    }
    if !t.copied.is_empty() {
        push(app, Undo::Copy(t.copied));
    }
}

pub fn push(app: &App, op: Undo) {
    let mut st = app.state.borrow_mut();
    st.undo.push(op);
    st.redo.clear();
}

/// What "Desfazer" would undo, as Explorer names it ("Desfazer Renomear").
pub fn label(app: &App) -> Option<String> {
    let shell32 = |id: u32| crate::i18n::system_string("shell32.dll", id);
    app.state.borrow().undo.last().map(|op| match op {
        Undo::Rename(..) => crate::i18n::tr("menu.rename").to_string(),
        Undo::Create(_) => crate::i18n::tr("cmd.new").to_string(),
        Undo::Delete(_) => crate::i18n::tr("context.delete").to_string(),
        // shell32's "Mover" and "Copiar".
        Undo::Move(_) => shell32(4145),
        Undo::Copy(_) => shell32(4146),
    })
}

pub fn can_undo(app: &App) -> bool {
    !app.state.borrow().undo.is_empty()
}

pub fn undo(app: &Rc<App>) {
    let Some(op) = app.state.borrow_mut().undo.pop() else { return };
    match op {
        Undo::Rename(renames) => {
            let done = rename_all(app, renames.iter().rev().map(|(from, to)| (to, from)));
            app.state.borrow_mut().redo.push(Undo::Rename(done.into_iter().rev().map(|(to, from)| (from, to)).collect()));
        }
        Undo::Create(path) => {
            delete(app, std::slice::from_ref(&path));
            app.state.borrow_mut().redo.push(Undo::Create(path));
        }
        Undo::Delete(paths) => {
            let _ = ops::restore_deleted(app.hwnd.get(), &paths);
            app.state.borrow_mut().redo.push(Undo::Delete(paths));
        }
        Undo::Move(pairs) => {
            let back: Vec<(String, String)> = pairs.iter().filter(|(_, to)| exists(to)).map(|(from, to)| (to.clone(), from.clone())).collect();
            let _ = ops::transfer_items(app.hwnd.get(), &back, false);
            app.state.borrow_mut().redo.push(Undo::Move(pairs));
        }
        Undo::Copy(pairs) => {
            // Only copies still beside their sources: never anything else.
            let copies: Vec<String> = pairs.iter().filter(|(from, to)| from != to && exists(from) && exists(to)).map(|(_, to)| to.clone()).collect();
            let _ = ops::delete_permanently(app.hwnd.get(), &copies);
            app.state.borrow_mut().redo.push(Undo::Copy(pairs));
        }
    }
    app.reload();
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// Ctrl+Y: does again what was undone: renames the items, brings a new item back from the
/// Recycle Bin (as Explorer does), or deletes the items again.
pub fn redo(app: &Rc<App>) {
    let Some(op) = app.state.borrow_mut().redo.pop() else { return };
    let again = match op {
        Undo::Rename(renames) => Undo::Rename(rename_all(app, renames.iter().map(|(from, to)| (from, to)))),
        Undo::Create(path) => {
            let _ = ops::restore_deleted(app.hwnd.get(), std::slice::from_ref(&path));
            Undo::Create(path)
        }
        Undo::Delete(paths) => {
            delete(app, &paths);
            Undo::Delete(paths)
        }
        Undo::Move(pairs) => {
            let again: Vec<(String, String)> = pairs.iter().filter(|(from, _)| exists(from)).cloned().collect();
            let _ = ops::transfer_items(app.hwnd.get(), &again, false);
            Undo::Move(pairs)
        }
        Undo::Copy(pairs) => {
            let again: Vec<(String, String)> = pairs.iter().filter(|(from, to)| exists(from) && !exists(to)).cloned().collect();
            let _ = ops::transfer_items(app.hwnd.get(), &again, true);
            Undo::Copy(pairs)
        }
    };
    app.state.borrow_mut().undo.push(again);
    app.reload();
}

/// Sends items of one folder to the Recycle Bin.
fn delete(app: &App, paths: &[String]) {
    let Some(first) = paths.first() else { return };
    let parent = std::path::Path::new(first).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let _ = ops::invoke_verb(app.hwnd.get(), &parent, paths, "delete", false);
}

/// Renames each item to the other path's name, selecting the last one after the reload;
/// returns the renames that worked, with the paths they ended at.
fn rename_all<'a>(app: &App, renames: impl Iterator<Item = (&'a String, &'a String)>) -> Vec<(String, String)> {
    let mut done = Vec::new();
    for (path, like) in renames {
        let name = std::path::Path::new(like).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Ok(next) = ops::rename(app.hwnd.get(), path, &name) {
            app.state.borrow_mut().after_load = Some(AfterLoad { select: next.clone(), rename: false });
            done.push((path.clone(), next));
        }
    }
    done
}
