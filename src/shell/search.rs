//! Search results the way Explorer's search box gets them: a shell search folder scoped to the
//! current location (Windows Search where indexed, a crawl elsewhere), matching item names.

use windows::core::PCWSTR;
use windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Search::Common::{COP_VALUE_CONTAINS, COP_WORD_STARTSWITH};
use windows::Win32::System::Search::{
    ConditionFactory, ICondition, IConditionFactory2, CONDITION_CREATION_DEFAULT,
};
use windows::Win32::UI::Shell::{
    ISearchFolderItemFactory, IShellItem, IShellItemArray, SHCreateItemFromIDList,
    SHCreateShellItemArrayFromShellItem, SearchFolderItemFactory,
};

use super::listing::{self, ShellEntry};
use super::{com_init, wide, Pidl};

/// The results, and the search folder's own icon (a magnifier, the tab's).
/// A query in Windows' Advanced Query Syntax, as a condition (keywords in the display
/// language: "tamanho:>1KB", "datamodificação:hoje").
fn aqs_condition(query: &str) -> Option<ICondition> {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    use windows::Win32::System::Search::{
        IQueryParser, IQueryParserManager, IQuerySolution, QueryParserManager, SQRO_DEFAULT,
    };
    use windows::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let manager: IQueryParserManager =
            CoCreateInstance(&QueryParserManager, None, CLSCTX_INPROC_SERVER).ok()?;
        let parser: IQueryParser = manager
            .CreateLoadedParser(windows::core::w!("SystemIndex"), GetUserDefaultUILanguage())
            .ok()?;
        manager.InitializeOptions(false, true, &parser).ok()?;
        let text = wide(query);
        let solution: IQuerySolution = parser.Parse(PCWSTR(text.as_ptr()), None).ok()?;
        let mut condition = None;
        solution.GetQuery(Some(&mut condition), None).ok()?;
        let now = GetLocalTime();
        solution
            .Resolve(condition.as_ref()?, SQRO_DEFAULT, Some(&now))
            .ok()
    }
}

/// "Opções de pesquisa": the folder only or its subfolders too, system files, file contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub current_folder_only: bool,
    pub system_files: bool,
    pub zipped_folders: bool,
    pub file_contents: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            current_folder_only: false,
            system_files: true,
            zipped_folders: archived_files(),
            file_contents: false,
        }
    }
}

const PREFERENCES: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Search\Preferences";

/// The native crawler reads the same persisted switch used by Explorer's menu.
pub fn archived_files() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(PREFERENCES)
        .and_then(|key| key.get_value::<u32, _>("ArchivedFiles"))
        .unwrap_or(1)
        != 0
}

pub fn set_archived_files(on: bool) -> Result<(), String> {
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(PREFERENCES)
        .map_err(|e| e.to_string())?;
    key.set_value("ArchivedFiles", &u32::from(on))
        .map_err(|e| e.to_string())?;
    notify_options_changed()
}

pub fn notify_options_changed() -> Result<(), String> {
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        // Shlwapi.lib imports SHGlobalCounterIncrement by ordinal 633; it has no
        // named export on Windows 11. The windows crate's named import won't load.
        let module =
            GetModuleHandleW(windows::core::w!("shlwapi.dll")).map_err(|e| e.to_string())?;
        let proc = GetProcAddress(module, windows::core::PCSTR(633usize as *const u8))
            .ok_or("Missing search-options counter")?;
        let increment: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(proc);
        increment(windows::Win32::UI::Shell::GLOBALCOUNTER_SEARCHOPTIONS.0);
    }
    Ok(())
}

pub fn folder(scope: &str, query: &str, options: Options) -> Result<Pidl, String> {
    com_init();
    unsafe {
        let factory: ISearchFolderItemFactory =
            CoCreateInstance(&SearchFolderItemFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.message())?;
        let name = wide(&crate::i18n::system_string("shell32.dll", 34133));
        factory
            .SetDisplayName(windows::core::PCWSTR(name.as_ptr()))
            .map_err(|e| e.message())?;

        let scope_pidl = Pidl::parse(scope)?;
        let scope_item: IShellItem =
            SHCreateItemFromIDList(scope_pidl.ptr()).map_err(|e| e.message())?;
        let scope_array: IShellItemArray =
            SHCreateShellItemArrayFromShellItem(&scope_item).map_err(|e| e.message())?;
        factory.SetScope(&scope_array).map_err(|e| e.message())?;

        // Property syntax ("tamanho:>1KB", "tipo:documento") goes through the Advanced Query
        // Syntax parser, in the user's language; plain words match names, as before.
        let parsed = if query.contains(':') {
            aqs_condition(query)
        } else {
            None
        };
        let condition: ICondition = match parsed {
            Some(c) => c,
            None => {
                let conditions: IConditionFactory2 =
                    CoCreateInstance(&ConditionFactory, None, CLSCTX_INPROC_SERVER)
                        .map_err(|e| e.message())?;
                let text = wide(query);
                let name: ICondition = conditions
                    .CreateStringLeaf(
                        &PKEY_ItemNameDisplay,
                        COP_WORD_STARTSWITH,
                        PCWSTR(text.as_ptr()),
                        PCWSTR::null(),
                        CONDITION_CREATION_DEFAULT,
                    )
                    .map_err(|e| e.message())?;
                if options.file_contents {
                    // "Conteúdo do arquivo": the name or what the file says.
                    use windows::Win32::Storage::EnhancedStorage::PKEY_Search_Contents;
                    let contents: ICondition = conditions
                        .CreateStringLeaf(
                            &PKEY_Search_Contents,
                            COP_VALUE_CONTAINS,
                            PCWSTR(text.as_ptr()),
                            PCWSTR::null(),
                            CONDITION_CREATION_DEFAULT,
                        )
                        .map_err(|e| e.message())?;
                    let pair = [Some(name), Some(contents)];
                    conditions
                        .CreateCompoundFromArray(
                            windows::Win32::System::Search::Common::CT_OR_CONDITION,
                            &pair,
                            CONDITION_CREATION_DEFAULT,
                        )
                        .map_err(|e| e.message())?
                } else {
                    name
                }
            }
        };
        factory.SetCondition(&condition).map_err(|e| e.message())?;

        Ok(Pidl(factory.GetIDList().map_err(|e| e.message())?))
    }
}

pub fn search(
    scope: &str,
    query: &str,
    options: Options,
) -> Result<(Vec<ShellEntry>, u32), String> {
    let results = folder(scope, query, options)?;
    unsafe {
        let icon = super::icons::icon_of_pidl(results.ptr()) & 0x00ff_ffff;
        let entries = listing::list_pidl(&results)?;
        let entries = entries
            .into_iter()
            .filter(|e| {
                // "Pasta atual": only what's right in the folder.
                let here = !options.current_folder_only
                    || std::path::Path::new(&e.path).parent().is_some_and(|p| {
                        p.to_string_lossy()
                            .eq_ignore_ascii_case(scope.trim_end_matches('\\'))
                    });
                // Without "Arquivos do sistema", the files marked system are left out.
                let system = options.system_files || !is_system(&e.path);
                here && system
            })
            .collect();
        Ok((entries, icon))
    }
}

/// Results arrive on the enumerator's STA, coalesced with the increasing native-view
/// cadence (200ms initially, +100ms per update, capped at 2s on a long crawl).
/// The callback transfers owned batches, never shell interfaces or a cloned full list.
pub fn stream(
    scope: &str,
    query: &str,
    options: Options,
    current: impl Fn() -> bool,
    mut emit: impl FnMut(Vec<ShellEntry>),
) -> Result<(), String> {
    if !current() {
        return Ok(());
    }
    let results = folder(scope, query, options)?;
    let mut batch = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut last = std::time::Instant::now();
    let mut interval = std::time::Duration::from_millis(200);
    listing::for_each_pidl(&results, |entry| {
        if !current() {
            return false;
        }
        let here = !options.current_folder_only
            || std::path::Path::new(&entry.path).parent().is_some_and(|p| {
                p.to_string_lossy()
                    .eq_ignore_ascii_case(scope.trim_end_matches('\\'))
            });
        if here
            && (options.system_files || !is_system(&entry.path))
            && seen.insert(entry.path.to_lowercase())
        {
            batch.push(entry);
        }
        if !batch.is_empty() && last.elapsed() >= interval {
            emit(std::mem::take(&mut batch));
            last = std::time::Instant::now();
            interval = (interval + std::time::Duration::from_millis(100))
                .min(std::time::Duration::from_millis(2000));
        }
        true
    })?;
    if current() && !batch.is_empty() {
        emit(batch);
    }
    Ok(())
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn cancelled_search_does_not_bind_its_obsolete_scope() {
        assert!(stream(
            "not a shell scope",
            "dll",
            Options::default(),
            || false,
            |_| panic!("cancelled batch")
        )
        .is_ok());
    }

    #[test]
    fn native_word_prefix_search_flushes_its_small_final_batch() {
        use std::{fs, path::PathBuf};
        struct Dummy(PathBuf);
        impl Drop for Dummy {
            fn drop(&mut self) {
                for name in [
                    "streambatch-alpha.txt",
                    "streambatch-beta.txt",
                    "midstreambatch.txt",
                ] {
                    let _ = fs::remove_file(self.0.join(name));
                }
                let _ = fs::remove_dir(&self.0);
            }
        }
        let root = std::env::temp_dir().join(format!(
            "codex-stream17-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let dummy = Dummy(root);
        for name in [
            "streambatch-alpha.txt",
            "streambatch-beta.txt",
            "midstreambatch.txt",
        ] {
            fs::write(dummy.0.join(name), b"stream17 unit dummy").unwrap();
        }
        let mut names = Vec::new();
        stream(
            &dummy.0.to_string_lossy(),
            "streambatch",
            Options::default(),
            || true,
            |batch| names.extend(batch.into_iter().map(|entry| entry.name)),
        )
        .unwrap();
        names.sort();
        assert_eq!(names, ["streambatch-alpha.txt", "streambatch-beta.txt"]);
    }
}

fn is_system(path: &str) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_SYSTEM != 0)
}

/// A property's enumerated values with their display texts (System.Kind: "document" /
/// "Documento"; System.Size: its ranges, "Pequeno (16 KB a 1 MB)"), in order.
pub fn enum_values(name: &str) -> Vec<(String, String)> {
    use windows::core::Interface;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyDescription, IPropertyEnumType, IPropertyEnumTypeList,
        PSGetPropertyDescriptionByName,
    };
    com_init();
    let mut out = Vec::new();
    unsafe {
        let w = wide(name);
        let mut desc: Option<IPropertyDescription> = None;
        if PSGetPropertyDescriptionByName(
            PCWSTR(w.as_ptr()),
            &IPropertyDescription::IID,
            &mut desc as *mut _ as *mut _,
        )
        .is_err()
        {
            return out;
        }
        let Some(desc) = desc else { return out };
        let Ok(list) = desc.GetEnumTypeList::<IPropertyEnumTypeList>() else {
            return out;
        };
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(item) = list.GetAt::<IPropertyEnumType>(i) else {
                continue;
            };
            let value = item
                .GetValue()
                .ok()
                .and_then(|v| PropVariantToStringAlloc(&v).ok())
                .map(|p| super::take_pwstr(p))
                .unwrap_or_default();
            let label = item
                .GetDisplayText()
                .map(|p| super::take_pwstr(p))
                .unwrap_or_default();
            out.push((value, label));
        }
    }
    out
}
