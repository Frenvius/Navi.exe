//! Library origins queried with STGM_READ; never commit or change a user definition.
use super::*;
use windows::core::GUID;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemArray, IShellLibrary, SHCreateItemFromParsingName, LFF_ALLITEMS,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};

#[derive(Clone, Debug)]
pub struct Origin {
    pub path: String,
    pub name: String,
    pub parent: String,
}

/// A filesystem child reached through an aggregated library. Keep its physical path
/// for file operations, but retain the library ancestor for breadcrumbs and Up.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Context {
    pub library: String,
    pub origin: String,
}
impl Context {
    pub fn contains(&self, path: &str) -> bool {
        let root = self.origin.trim_end_matches('\\').to_lowercase();
        path.to_lowercase().strip_prefix(&root).is_some_and(|tail| tail.starts_with('\\'))
    }
    pub fn crumbs(&self, path: &str) -> Option<Vec<listing::Crumb>> {
        if !self.contains(path) { return None; }
        let mut chain = listing::crumbs(&self.library).ok()?;
        let children = listing::crumbs(path).ok()?;
        chain.extend(children.into_iter().filter(|c| self.contains(&c.id)));
        Some(chain)
    }
}
pub fn is_library(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".library-ms")
}
pub fn folder_type(path:&str)->Option<GUID>{
    com_init();unsafe{
        let library:IShellLibrary=CoCreateInstance(&GUID::from_u128(0xd9b3211d_e57f_4426_aaef_30a806add397),None,CLSCTX_INPROC_SERVER).ok()?;
        let name=wide(path);let item:IShellItem=SHCreateItemFromParsingName(PCWSTR(name.as_ptr()),None).ok()?;
        library.LoadLibraryFromItem(&item,0).ok()?;library.GetFolderType().ok()
    }
}
pub fn origins(path: &str) -> Vec<Origin> {
    com_init();
    let read = || -> windows::core::Result<Vec<Origin>> {
        unsafe {
            let library: IShellLibrary = CoCreateInstance(
                &GUID::from_u128(0xd9b3211d_e57f_4426_aaef_30a806add397),
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            let path = wide(path);
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None)?;
            library.LoadLibraryFromItem(&item, 0)?;
            let folders: IShellItemArray = library.GetFolders(LFF_ALLITEMS)?;
            let mut result = Vec::new();
            for index in 0..folders.GetCount()? {
                let item = folders.GetItemAt(index)?;
                let path = item
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .map(|p| take_pwstr(p))
                    .unwrap_or_default();
                if path.is_empty() {
                    continue;
                }
                let name = take_pwstr(item.GetDisplayName(SIGDN_NORMALDISPLAY)?);
                let parent = listing::folder_path_display(&path).unwrap_or_default();
                result.push(Origin { path, name, parent });
            }
            Ok(result)
        }
    };
    read().unwrap_or_default()
}

/// Resolve the library's existing default destination without changing it.
pub fn save_folder(path: &str) -> Option<String> {
    if !is_library(path) { return None; }
    com_init();
    unsafe {
        let library:IShellLibrary=CoCreateInstance(&GUID::from_u128(0xd9b3211d_e57f_4426_aaef_30a806add397),None,CLSCTX_INPROC_SERVER).ok()?;
        let name=wide(path);
        let item:IShellItem=SHCreateItemFromParsingName(PCWSTR(name.as_ptr()),None).ok()?;
        library.LoadLibraryFromItem(&item,0).ok()?;
        let folder:IShellItem=library.GetDefaultSaveFolder(windows::Win32::UI::Shell::DSFT_DETECT).ok()?;
        Some(take_pwstr(folder.GetDisplayName(SIGDN_FILESYSPATH).ok()?))
    }
}
pub fn origin_for(path: &str, origins: &[Origin]) -> Option<Origin> {
    let path = path.to_lowercase();
    origins
        .iter()
        .filter(|o| {
            let root = o.path.to_lowercase();
            path == root
                || path
                    .strip_prefix(&root)
                    .is_some_and(|tail| tail.starts_with('\\'))
        })
        .max_by_key(|o| o.path.len())
        .cloned()
}
pub fn is_location(key: &windows::Win32::Foundation::PROPERTYKEY) -> bool {
    static KEY: std::sync::OnceLock<Option<windows::Win32::Foundation::PROPERTYKEY>> =
        std::sync::OnceLock::new();
    KEY.get_or_init(|| super::columns::key("System.ItemSearchLocation"))
        .as_ref()
        == Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregates_duplicate_names_without_changing_definition_and_retains_library_ancestor() {
        com_init();
        let root = std::env::temp_dir().join(format!("codex-library-unit-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        let a=root.join("A"); let b=root.join("B");
        let definition=root.join("Library.library-ms");
        struct Fixture(Vec<std::path::PathBuf>,Vec<std::path::PathBuf>);
        impl Drop for Fixture { fn drop(&mut self) {
            for p in &self.0 { if p.is_file() {std::fs::remove_file(p).unwrap();} }
            for p in &self.1 {if p.is_dir() {std::fs::remove_dir(p).unwrap();}}
        }}
        let _cleanup=Fixture(vec![a.join("same.txt"),b.join("same.txt"),definition.clone()],vec![a.clone(),b.clone(),root.clone()]);
        for folder in [&a,&b] {std::fs::create_dir(folder).unwrap();std::fs::write(folder.join("same.txt"),b"owned dummy").unwrap();}
        unsafe {
            let library:IShellLibrary=CoCreateInstance(&GUID::from_u128(0xd9b3211d_e57f_4426_aaef_30a806add397),None,CLSCTX_INPROC_SERVER).unwrap();
            for folder in [&a,&b] {
                let name=wide(&folder.to_string_lossy());
                let item:IShellItem=SHCreateItemFromParsingName(PCWSTR(name.as_ptr()),None).unwrap();
                library.AddFolder(&item).unwrap();
            }
            library.SetFolderType(&windows::Win32::UI::Shell::FOLDERTYPEID_Documents).unwrap();
            let name=wide(&root.to_string_lossy());
            let folder:IShellItem=SHCreateItemFromParsingName(PCWSTR(name.as_ptr()),None).unwrap();
            library.Save(&folder,windows::core::w!("Library"),windows::Win32::UI::Shell::LSF_FAILIFTHERE).unwrap();
        }
        let before=std::fs::read(&definition).unwrap();
        let path=definition.to_string_lossy();
        let sort=vec![(windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay,false),(windows::Win32::Storage::EnhancedStorage::PKEY_Size,true)];
        super::viewstate::write(&path,&super::viewstate::ViewState{logical_mode:Some(3),icon_size:Some(48),sort:sort.clone(),..Default::default()});
        assert_eq!(super::viewstate::native_view_for_test(&path).unwrap().sort,sort,"Windows must read both saved sorting levels");
        let origins=origins(&path);
        assert_eq!(origins.len(),2);
        let entries=listing::list(&path).unwrap();
        assert_eq!(entries.iter().filter(|e|e.name=="same.txt").count(),2);
        assert!(entries.iter().all(|e|e.library_origin.is_some()&&!e.child.is_empty()&&!e.absolute.is_empty()));
        assert_ne!(entries[0].path,entries[1].path);
        let context=Context{library:path.into(),origin:a.to_string_lossy().into()};
        let child=a.join("same.txt").to_string_lossy().to_string();
        assert!(context.contains(&child));
        assert!(!context.contains(&format!("{}other\\same.txt",a.display())));
        let crumbs=context.crumbs(&child).unwrap();
        assert!(crumbs[crumbs.len()-2].id.eq_ignore_ascii_case(&context.library));
        assert_eq!(std::fs::read(&definition).unwrap(),before);
    }
}
