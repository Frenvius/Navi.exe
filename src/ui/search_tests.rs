use super::*;
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::*;

#[test]
fn streaming_search_uses_the_list_worker_and_rejects_cancelled_batches() {
    let root = std::env::temp_dir().join(format!("navi-search18-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("native18 alpha.txt"), b"owned small search dummy").unwrap();
    std::fs::write(root.join("native18 large.txt"), vec![b'x'; 4096]).unwrap();
    std::fs::write(root.join("midnative18.txt"), b"excluded mid-word dummy").unwrap();
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("STATIC"),
            w!("NativeSearch18Test"),
            Default::default(),
            0,
            0,
            1,
            1,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
        .unwrap()
    };
    let worker = super::super::list::Worker::start().unwrap();
    let query = crate::app::ids::search_id(root.to_str().unwrap(), "native18");
    let mut list =
        super::super::list::List::search_start(&worker, hwnd, query, None, Default::default())
            .unwrap();
    let mut scroll = super::super::scroll::State::default();
    let deadline = Instant::now() + Duration::from_secs(15);
    while list.search.busy() {
        list.receive(&mut scroll).unwrap();
        assert!(
            Instant::now() < deadline,
            "native search completion timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let folder = list.folder.as_ref().unwrap();
    assert_eq!(folder.entries.len(), 2);
    assert_eq!(list.search.workers(), 0);
    assert!(folder.view.search_ranked);
    assert_eq!(folder.view.mode, 8);
    assert!(folder
        .entries
        .iter()
        .all(|entry| !entry.absolute.is_empty()));
    list.set_mode(6, 16).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while list.rows.len() != 2 {
        list.receive(&mut scroll).unwrap();
        list.request(0..2).unwrap();
        assert!(Instant::now() < deadline, "Details rows did not load");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(list.rows.values().all(|row| row.picture.is_none()));
    list.set_mode(8, 32).unwrap();
    list.prepare_grid((889.0, 559.0)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while list.rows.len() != 2 || list.rows.values().any(|row| row.picture.is_none()) {
        list.receive(&mut scroll).unwrap();
        list.request(0..2).unwrap();
        assert!(
            Instant::now() < deadline,
            "Content retained obsolete Details payloads"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(list
        .rows
        .values()
        .all(|row| !row.picture.as_ref().unwrap().fields.is_empty()));
    drop(list);
    let query = crate::app::ids::search_id(root.to_str().unwrap(), "tamanho:>1KB");
    let mut list =
        super::super::list::List::search_start(&worker, hwnd, query, None, Default::default())
            .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while list.search.busy() {
        list.receive(&mut scroll).unwrap();
        assert!(Instant::now() < deadline, "native AQS completion timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(list.folder.as_ref().unwrap().entries.len(), 1);
    assert_eq!(
        list.folder.as_ref().unwrap().entries[0].name,
        "native18 large.txt"
    );
    drop(list);
    let query = crate::app::ids::search_id(root.to_str().unwrap(), "native18");
    let mut list =
        super::super::list::List::search_start(&worker, hwnd, query, None, Default::default())
            .unwrap();
    list.search.cancel();
    let deadline = Instant::now() + Duration::from_secs(15);
    while list.folder.is_none() || list.search.workers() != 0 {
        list.receive(&mut scroll).unwrap();
        assert!(
            Instant::now() < deadline,
            "cancelled native enumerator did not stop"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    list.receive(&mut scroll).unwrap();
    assert!(list.folder.as_ref().unwrap().entries.is_empty());
    drop(list);
    drop(worker);
    unsafe {
        DestroyWindow(hwnd).unwrap();
    }
    for name in [
        "native18 alpha.txt",
        "native18 large.txt",
        "midnative18.txt",
    ] {
        std::fs::remove_file(root.join(name)).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}
