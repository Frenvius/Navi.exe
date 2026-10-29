use std::cell::RefCell;
use windows::core::{Error, Interface, Result, HSTRING};
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile};
use windows::Win32::Foundation::{E_FAIL, E_POINTER, HWND};
use windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED, RO_INIT_TYPE,
};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;

struct Runtime;

impl Runtime {
    fn initialize(mode: RO_INIT_TYPE) -> Result<Self> {
        unsafe {
            RoInitialize(mode)?;
        }
        Ok(Self)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

struct Share {
    window: usize,
    manager: DataTransferManager,
    token: i64,
    _runtime: Runtime,
}

impl Drop for Share {
    fn drop(&mut self) {
        if let Err(error) = self.manager.RemoveDataRequested(self.token) {
            eprintln!("share event cleanup: {error}");
        }
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Share>> = const {RefCell::new(None)};
}

pub fn close(hwnd: HWND) {
    let previous = ACTIVE.with(|slot| {
        let mut current = slot.borrow_mut();
        if current
            .as_ref()
            .is_some_and(|share| share.window == hwnd.0 as usize)
        {
            current.take()
        } else {
            None
        }
    });
    drop(previous);
}

pub fn show(hwnd: HWND, paths: &[String], title: &str) -> Result<()> {
    let previous = ACTIVE.with(|slot| slot.borrow_mut().take());
    drop(previous);
    let runtime = Runtime::initialize(RO_INIT_SINGLETHREADED)?;
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let manager: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
    let paths = paths.to_vec();
    let title = HSTRING::from(title);
    let handler =
        TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
            let request = args
                .as_ref()
                .ok_or_else(|| Error::from_hresult(E_POINTER))?
                .Request()?;
            let data = request.Data()?;
            data.Properties()?.SetTitle(&title)?;
            let deferral = request.GetDeferral()?;
            let pending = request.clone();
            let completion = deferral.clone();
            let paths = paths.clone();
            let started = std::thread::Builder::new()
                .name("share-items".into())
                .spawn(move || {
                    let runtime = Runtime::initialize(RO_INIT_MULTITHREADED);
                    let result = (|| -> Result<()> {
                        let _runtime = runtime.as_ref().map_err(Clone::clone)?;
                        data.SetStorageItemsReadOnly(&load_items(&paths)?)
                    })();
                    if let Err(error) = result {
                        eprintln!("share file data: {error}");
                        if let Err(failure) =
                            pending.FailWithDisplayText(&HSTRING::from(error.message()))
                        {
                            eprintln!("share error display: {failure}");
                        }
                    }
                    if let Err(error) = completion.Complete() {
                        eprintln!("share request completion: {error}");
                    }
                    drop(runtime);
                });
            if let Err(error) = started {
                let failure = Error::new(E_FAIL, error.to_string());
                request.FailWithDisplayText(&HSTRING::from(failure.message()))?;
                deferral.Complete()?;
                return Err(failure);
            }
            Ok(())
        });
    let token = manager.DataRequested(&handler)?;
    ACTIVE.with(|slot| {
        *slot.borrow_mut() = Some(Share {
            window: hwnd.0 as usize,
            manager,
            token,
            _runtime: runtime,
        })
    });
    if let Err(error) = unsafe { interop.ShowShareUIForWindow(hwnd) } {
        close(hwnd);
        return Err(error);
    }
    Ok(())
}

fn load_items(paths: &[String]) -> Result<windows_collections::IIterable<IStorageItem>> {
    paths
        .iter()
        .map(|path| {
            StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?
                .get()?
                .cast::<IStorageItem>()
                .map(Some)
        })
        .collect::<Result<Vec<_>>>()
        .map(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::ApplicationModel::DataTransfer::DataPackage;

    #[test]
    fn shared_file_package_contains_exact_selected_paths_and_rejects_missing_files() {
        std::thread::spawn(|| -> Result<()> {
            let _runtime = Runtime::initialize(RO_INIT_MULTITHREADED)?;
            let root = std::env::temp_dir().join(format!("sw-native-share-{}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            let paths = [root.join("alpha.txt"), root.join("beta.txt")]
                .map(|path| path.to_string_lossy().into_owned());
            for path in &paths {
                std::fs::write(path, b"task10 dummy").unwrap();
            }
            let data = DataPackage::new()?;
            data.SetStorageItemsReadOnly(&load_items(&paths)?)?;
            let items = data.GetView()?.GetStorageItemsAsync()?.get()?;
            assert_eq!(items.Size()?, 2);
            for (index, path) in paths.iter().enumerate() {
                assert_eq!(items.GetAt(index as u32)?.Path()?.to_string(), *path);
            }
            assert!(
                load_items(&[root.join("missing.txt").to_string_lossy().into_owned()]).is_err()
            );
            for path in paths {
                std::fs::remove_file(path).unwrap();
            }
            std::fs::remove_dir(root).unwrap();
            Ok(())
        })
        .join()
        .unwrap()
        .unwrap();
    }
}
