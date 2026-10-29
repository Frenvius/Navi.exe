use notify::{EventKind, RecursiveMode, Watcher};

pub fn listen(
    paths: &[String],
    mut changed: impl FnMut(notify::Result<notify::Event>) + Send + 'static,
) -> notify::Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if !event
            .as_ref()
            .is_ok_and(|event| matches!(event.kind, EventKind::Access(_)))
        {
            changed(event);
        }
    })?;
    let mut watched = false;
    let mut failure = None;
    for path in paths {
        if !std::path::Path::new(path).is_dir() {
            continue;
        }
        match watcher.watch(std::path::Path::new(path), RecursiveMode::NonRecursive) {
            Ok(()) => watched = true,
            Err(error) => {
                eprintln!("watch {path}: {error}");
                failure = Some(error);
            }
        }
    }
    if !watched {
        return Err(
            failure.unwrap_or_else(|| notify::Error::generic("no existing directories to watch"))
        );
    }
    Ok(watcher)
}

#[derive(Default)]
pub struct Renames {
    pending: Option<notify::Event>,
}

impl Renames {
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    pub fn flush(&mut self) -> Option<notify::Event> {
        self.pending
            .take()
            .map(|event| event.set_kind(EventKind::Remove(notify::event::RemoveKind::Any)))
    }

    pub fn push(&mut self, mut event: notify::Event) -> Vec<notify::Event> {
        use notify::event::{ModifyKind, RenameMode};
        let mut ready = Vec::new();
        if matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::To))
        ) {
            if let Some(from) = self.pending.take() {
                let mut paths = from.paths;
                paths.extend(event.paths);
                event.paths = paths;
                ready.push(event.set_kind(EventKind::Modify(ModifyKind::Name(RenameMode::Both))));
                return ready;
            }
        }
        if let Some(from) = self.flush() {
            ready.push(from);
        }
        if matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::From))
        ) {
            self.pending = Some(event);
        } else {
            ready.push(event);
        }
        ready
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use notify::event::{ModifyKind, RenameMode};

    #[test]
    fn separate_windows_rename_records_form_one_identity_change_and_orphans_expire() {
        let event = |mode, path: &str| {
            notify::Event::new(EventKind::Modify(ModifyKind::Name(mode))).add_path(path.into())
        };
        let mut renames = Renames::default();
        assert!(renames.push(event(RenameMode::From, "old.txt")).is_empty());
        assert!(renames.waiting());
        let paired = renames.push(event(RenameMode::To, "new.txt"));
        assert_eq!(paired.len(), 1);
        assert!(matches!(
            paired[0].kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::Both))
        ));
        assert_eq!(
            paired[0].paths,
            vec![
                std::path::PathBuf::from("old.txt"),
                std::path::PathBuf::from("new.txt")
            ]
        );
        assert!(!renames.waiting());
        renames.push(event(RenameMode::From, "moved-out.txt"));
        let expired = renames.flush().unwrap();
        assert!(matches!(expired.kind, EventKind::Remove(_)));
        assert_eq!(
            expired.paths,
            vec![std::path::PathBuf::from("moved-out.txt")]
        );
        assert!(!renames.waiting());
    }
}
