use windows::Win32::Foundation::HWND;

use crate::shell::ops;

#[derive(Clone)]
pub enum Undo {
    Rename(Vec<(String, String)>),
    Create(String),
    Delete(Vec<String>),
    Move(Vec<(String, String)>),
    Copy(Vec<(String, String)>),
}

pub struct Applied {
    pub operation: Undo,
    pub select: Option<String>,
}

pub fn transfers(transfer: ops::Transfer) -> Vec<Undo> {
    let mut operations = Vec::new();
    if !transfer.moved.is_empty() {
        operations.push(Undo::Move(transfer.moved));
    }
    if !transfer.copied.is_empty() {
        operations.push(Undo::Copy(transfer.copied));
    }
    operations
}

pub fn operation_label(operation: &Undo) -> String {
    match operation {
        Undo::Rename(_) => crate::i18n::tr("menu.rename").to_owned(),
        Undo::Create(_) => crate::i18n::tr("cmd.new").to_owned(),
        Undo::Delete(_) => crate::i18n::tr("context.delete").to_owned(),
        Undo::Move(_) => crate::i18n::system_string("shell32.dll", 4145),
        Undo::Copy(_) => crate::i18n::system_string("shell32.dll", 4146),
    }
}

pub fn undo_operation(hwnd: HWND, operation: Undo) -> Result<Applied, String> {
    let mut select = None;
    let operation = match operation {
        Undo::Rename(pairs) => {
            let done = rename_all(hwnd, pairs.iter().rev().map(|(from, to)| (to, from)))?;
            select = done.last().map(|(_, path)| path.clone());
            Undo::Rename(
                done.into_iter()
                    .rev()
                    .map(|(to, from)| (from, to))
                    .collect(),
            )
        }
        Undo::Create(path) => {
            delete(hwnd, std::slice::from_ref(&path))?;
            Undo::Create(path)
        }
        Undo::Delete(paths) => {
            ops::restore_deleted(hwnd, &paths)?;
            select = paths.first().cloned();
            Undo::Delete(paths)
        }
        Undo::Move(pairs) => {
            let back: Vec<_> = pairs
                .iter()
                .filter(|(_, to)| exists(to))
                .map(|(from, to)| (to.clone(), from.clone()))
                .collect();
            ops::transfer_items(hwnd, &back, false)?;
            Undo::Move(pairs)
        }
        Undo::Copy(pairs) => {
            let copies: Vec<_> = pairs
                .iter()
                .filter(|(from, to)| from != to && exists(from) && exists(to))
                .map(|(_, to)| to.clone())
                .collect();
            ops::delete_permanently(hwnd, &copies)?;
            Undo::Copy(pairs)
        }
    };
    Ok(Applied { operation, select })
}

pub fn redo_operation(hwnd: HWND, operation: Undo) -> Result<Applied, String> {
    let mut select = None;
    let operation = match operation {
        Undo::Rename(pairs) => {
            let done = rename_all(hwnd, pairs.iter().map(|(from, to)| (from, to)))?;
            select = done.last().map(|(_, path)| path.clone());
            Undo::Rename(done)
        }
        Undo::Create(path) => {
            ops::restore_deleted(hwnd, std::slice::from_ref(&path))?;
            select = Some(path.clone());
            Undo::Create(path)
        }
        Undo::Delete(paths) => {
            delete(hwnd, &paths)?;
            Undo::Delete(paths)
        }
        Undo::Move(pairs) => {
            let again: Vec<_> = pairs
                .iter()
                .filter(|(from, _)| exists(from))
                .cloned()
                .collect();
            ops::transfer_items(hwnd, &again, false)?;
            Undo::Move(pairs)
        }
        Undo::Copy(pairs) => {
            let again: Vec<_> = pairs
                .iter()
                .filter(|(from, to)| exists(from) && !exists(to))
                .cloned()
                .collect();
            ops::transfer_items(hwnd, &again, true)?;
            Undo::Copy(pairs)
        }
    };
    Ok(Applied { operation, select })
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

fn delete(hwnd: HWND, paths: &[String]) -> Result<(), String> {
    let Some(first) = paths.first() else {
        return Ok(());
    };
    let parent = std::path::Path::new(first)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    ops::invoke_verb(hwnd, &parent.to_string_lossy(), paths, "delete", false)
}

fn rename_all<'a>(
    hwnd: HWND,
    pairs: impl Iterator<Item = (&'a String, &'a String)>,
) -> Result<Vec<(String, String)>, String> {
    let mut done = Vec::new();
    let mut failure = None;
    for (path, like) in pairs {
        let name = std::path::Path::new(like)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match ops::rename(hwnd, path, &name) {
            Ok(next) => done.push((path.clone(), next)),
            Err(error) => {
                eprintln!("rename history {path}: {error}");
                failure = Some(error);
            }
        }
    }
    if done.is_empty() {
        if let Some(error) = failure {
            return Err(error);
        }
    }
    Ok(done)
}
