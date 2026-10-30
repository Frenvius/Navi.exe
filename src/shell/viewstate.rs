use windows::core::{IUnknown, Interface, GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use windows::Win32::System::Com::{CoCreateInstance, IStream, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VT_UI4, VT_UNKNOWN};
use windows::Win32::UI::Shell::Common::SHELLDETAILS;
use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
use windows::Win32::UI::Shell::{
    IKnownFolderManager, IShellFolder2, KnownFolderManager, SHCreateMemStream,
    SHGetViewStatePropertyBag, StrRetToStrW, SIGDN_FILESYSPATH,
};

use super::{com_init, wide, Pidl};
#[path = "viewstate_stream.rs"]
mod streamstore;

/// Per user and per folder; without the shell's generic defaults, which we fill in from
/// the folder type instead.
const SHGVSPB_FOLDERNODEFAULTS: u32 = 0x8000_0005;
const SHGVSPB_FOLDER: u32 = 0x5;
const GENERIC: GUID = GUID::from_u128(0x5c4f28b5_f869_4e84_8e60_f11db97c5cc7);
const FOLDER_TYPES: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\FolderTypes";

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    /// FOLDERLOGICALVIEWMODE: 1 details, 2 tiles, 3 icons, 4 list, 5 content.
    pub logical_mode: Option<u32>,
    pub icon_size: Option<u32>,
    /// Sort columns, first the primary, with whether each is descending.
    pub sort: Vec<(PROPERTYKEY, bool)>,
    /// The grouping column and whether groups go descending; `Some(None)` is "(Nenhum)".
    pub group: Option<Option<(PROPERTYKEY, bool)>>,
    /// The columns shown, in order, with their widths (pixels at 100%).
    pub columns: Vec<(PROPERTYKEY, u32)>,
    /// The folder type's columns ("ColumnList"): the header menu's list, with whether each
    /// shows by default and its width in characters when the type sets one.
    pub type_columns: Vec<(PROPERTYKEY, bool, Option<u32>)>,
    /// Header texts a folder gives its own columns ("Local Original"), over the property
    /// system's ("Local original").
    pub labels: Vec<(PROPERTYKEY, String)>,
    /// Columns a folder of its own right-aligns (This PC's Tamanho Total, Espaço livre).
    pub right: Vec<PROPERTYKEY>,
}

pub fn read(path: &str) -> ViewState {
    com_init();
    let Ok(pidl) = Pidl::parse(path) else {
        return ViewState::default();
    };
    let kind = folder_type(&pidl);
    let mut state = defaults(&kind);
    let bag_kind = persistence_type(&pidl, kind);
    if let Some(values) = type_default_bytes(bag_kind).and_then(|b| parse_storage(&b)) {
        apply_saved(&Saved::Stream(values), &mut state);
    }
    // A folder outside the file system with no type of its own (the Recycle Bin, Network) has
    // its own columns, not the generic type's.
    let own_view = (kind == GENERIC && pidl.name(SIGDN_FILESYSPATH).is_empty())
        || kind == windows::Win32::UI::Shell::FOLDERTYPEID_CompressedFolder
        || super::network::is_computer(path)
        || super::library::is_library(path);
    if own_view {
        let own = folder_columns(&pidl);
        if !own.is_empty() {
            state.labels = own
                .iter()
                .map(|(k, _, _, label, _)| (*k, label.clone()))
                .filter(|(_, l)| !l.is_empty())
                .collect();
            state.right = own.iter().filter(|c| c.4).map(|c| c.0).collect();
            state.type_columns = own
                .into_iter()
                .map(|(k, shown, chars, _, _)| (k, shown, chars))
                .collect();
        }
    }
    let bag_name = wide(&format!(r"Shell\{{{bag_kind:?}}}"));
    let bag: Option<IPropertyBag> = unsafe {
        SHGetViewStatePropertyBag(
            Some(pidl.0),
            PCWSTR(bag_name.as_ptr()),
            SHGVSPB_FOLDERNODEFAULTS,
        )
        .ok()
    };
    // Recycle Bin can have both a legacy stream and its newer internal-type bag.
    // Once a modern bag has a mode, Explorer uses it ahead of the old stream.
    let saved = if bag
        .as_ref()
        .and_then(|b| read_u32(b, "LogicalViewMode"))
        .is_some()
    {
        Saved::Bag(bag.unwrap())
    } else if let Some(values) = stream_view(&pidl) {
        Saved::Stream(values)
    } else if let Some(bag) = bag {
        Saved::Bag(bag)
    } else {
        return state;
    };
    // Nothing saved for such a folder: the view it gives itself (Network: tiles grouped by
    // category), as Explorer's own view of it starts. A saved view without a grouping keeps
    // the folder's own (This PC's, by kind).
    let no_mode = saved.u32("LogicalViewMode").is_none();
    let no_group = saved.string("GroupByKey:FMTID").is_none();
    if own_view && (no_mode || no_group) {
        if let Some(own) = folder_view(&pidl) {
            if no_mode {
                state.logical_mode = own.logical_mode;
                state.icon_size = own.icon_size;
                state.sort = own.sort;
                state.columns = own.columns;
            }
            state.group = own.group;
        }
    }
    apply_saved(&saved, &mut state);
    state
}

fn apply_saved(saved: &Saved, state: &mut ViewState) {
    if let Some(mode) = saved.u32("LogicalViewMode") {
        state.logical_mode = Some(mode);
        state.icon_size = saved.u32("IconSize").or(state.icon_size);
    }
    if let Some(bytes) = saved.stream("Sort") {
        let sort = parse_sort(&bytes);
        if !sort.is_empty() {
            state.sort = sort;
        }
    }
    if let (Some(fmtid), Some(pid)) = (
        saved.string("GroupByKey:FMTID"),
        saved.u32("GroupByKey:PID"),
    ) {
        let fmtid =
            GUID::try_from(fmtid.trim_matches(|c| c == '{' || c == '}')).unwrap_or_default();
        let descending = saved
            .u32("GroupByDirection")
            .map(|d| d as i32 == -1)
            .unwrap_or(false);
        state.group = Some(
            (fmtid != GUID::zeroed() && pid != 0)
                .then_some((PROPERTYKEY { fmtid, pid }, descending)),
        );
    }
    if let Some(bytes) = saved.stream("ColInfo") {
        state.columns = parse_columns(&bytes);
    }
}

/// Apply to Folders uses this native serialized per-type default, then clears that
/// type's per-folder bags. Do not confuse it with a per-PIDL legacy ViewView2 stream.
fn type_default_bytes(kind: GUID) -> Option<Vec<u8>> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Streams\Defaults")
        .ok()?
        .get_raw_value(&format!("{{{kind:?}}}"))
        .ok()
        .map(|v| v.bytes)
}

pub fn default_fingerprint(path: &str) -> Option<Vec<u8>> {
    let pidl = Pidl::parse(path).ok()?;
    type_default_bytes(persistence_type(&pidl, folder_type(&pidl)))
}

/// The view a folder sets up for itself: Explorer's view (DefView) of it in a hidden
/// ExplorerBrowser, read before it lists anything (a few tens of milliseconds). Its mode, sort,
/// grouping and shown columns with their widths.
fn folder_view(pidl: &Pidl) -> Option<ViewState> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::Shell::{
        ExplorerBrowser, IColumnManager, IExplorerBrowser, IFolderView2, CM_COLUMNINFO,
        CM_ENUM_VISIBLE, CM_MASK_WIDTH, FOLDERSETTINGS, FVM_AUTO, FVM_CONTENT, FVM_DETAILS,
        FVM_LIST, FVM_TILE, SBSP_ABSOLUTE, SORTCOLUMN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP,
    };
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            None,
            WS_POPUP,
            0,
            0,
            800,
            600,
            None,
            None,
            None,
            None,
        )
        .ok()?;
        let read = || -> Option<ViewState> {
            let browser: IExplorerBrowser =
                CoCreateInstance(&ExplorerBrowser, None, CLSCTX_INPROC_SERVER).ok()?;
            // Match Explorer's Shell store; ExplorerBrowser otherwise reads ComDlg.
            browser.SetPropertyBag(windows::core::w!("Shell")).ok()?;
            let settings = FOLDERSETTINGS {
                ViewMode: FVM_AUTO.0 as u32,
                fFlags: 0,
            };
            browser
                .Initialize(
                    hwnd,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: 800,
                        bottom: 600,
                    },
                    Some(&settings),
                )
                .ok()?;
            let result = (|| {
                browser.BrowseToIDList(pidl.0, SBSP_ABSOLUTE).ok()?;
                let view: IFolderView2 = browser.GetCurrentView().ok()?;
                let mut state = ViewState::default();
                let mut mode = Default::default();
                let mut size = 0;
                view.GetViewModeAndIconSize(&mut mode, &mut size).ok()?;
                state.logical_mode = Some(match mode {
                    m if m == FVM_DETAILS => 1,
                    m if m == FVM_TILE => 2,
                    m if m == FVM_LIST => 4,
                    m if m == FVM_CONTENT => 5,
                    _ => 3,
                });
                state.icon_size = Some(size as u32);
                let count = view.GetSortColumnCount().unwrap_or(0).max(0) as usize;
                let mut sorts = vec![SORTCOLUMN::default(); count];
                if count > 0 && view.GetSortColumns(&mut sorts).is_ok() {
                    state.sort = sorts
                        .iter()
                        .map(|s| (s.propkey, s.direction.0 < 0))
                        .collect();
                }
                let mut key = PROPERTYKEY::default();
                let mut ascending = BOOL::default();
                if view.GetGroupBy(&mut key, Some(&mut ascending)).is_ok() {
                    state.group =
                        Some((key.fmtid != GUID::zeroed()).then_some((key, !ascending.as_bool())));
                }
                let columns: IColumnManager = view.cast().ok()?;
                let n = columns.GetColumnCount(CM_ENUM_VISIBLE).unwrap_or(0) as usize;
                let mut keys = vec![PROPERTYKEY::default(); n];
                if n > 0 && columns.GetColumns(CM_ENUM_VISIBLE, &mut keys).is_ok() {
                    state.columns = keys
                        .iter()
                        .map(|k| {
                            let mut info = CM_COLUMNINFO {
                                cbSize: std::mem::size_of::<CM_COLUMNINFO>() as u32,
                                dwMask: CM_MASK_WIDTH.0 as u32,
                                ..Default::default()
                            };
                            let _ = columns.GetColumnInfo(k, &mut info);
                            (*k, info.uWidth)
                        })
                        .collect();
                }
                Some(state)
            })();
            let _ = browser.Destroy();
            result
        };
        let state = read();
        let _ = DestroyWindow(hwnd);
        state
    }
}

#[cfg(test)]
pub(crate) fn native_view_for_test(path: &str) -> Option<ViewState> {
    folder_view(&Pidl::parse(path).ok()?)
}

/// Where a folder's view was saved: its bag, or Explorer's older "Streams" store, which it
/// keeps the Recycle Bin's in.
enum Saved {
    Bag(IPropertyBag),
    Stream(Vec<(String, Value)>),
}

/// A value of a serialized property storage.
enum Value {
    Number(u32),
    Text(String),
    Bytes(Vec<u8>),
}

impl Saved {
    fn find(&self, name: &str) -> Option<&Value> {
        match self {
            Saved::Stream(values) => values.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            Saved::Bag(_) => None,
        }
    }

    fn u32(&self, name: &str) -> Option<u32> {
        match self {
            Saved::Bag(bag) => read_u32(bag, name),
            _ => match self.find(name)? {
                Value::Number(n) => Some(*n),
                _ => None,
            },
        }
    }

    fn string(&self, name: &str) -> Option<String> {
        match self {
            Saved::Bag(bag) => read_string(bag, name),
            _ => match self.find(name)? {
                Value::Text(t) => Some(t.clone()),
                _ => None,
            },
        }
    }

    fn stream(&self, name: &str) -> Option<Vec<u8>> {
        match self {
            Saved::Bag(bag) => read_stream(bag, name),
            _ => match self.find(name)? {
                Value::Bytes(b) => Some(b.clone()),
                _ => None,
            },
        }
    }
}

/// A folder's view in the "Streams" store: its absolute PIDL is a value of StreamMRU, and
/// Streams\<that value's name>\ViewView2 holds a serialized property storage with the bag's
/// values (LogicalViewMode, Sort, ColInfo...).
fn stream_view(pidl: &Pidl) -> Option<Vec<(String, Value)>> {
    parse_storage(&streamstore::Store::open(pidl, false)?.bytes())
}

/// The named values of a serialized property storage ("1SPS", the string-named section):
/// each a size, its name's size, a byte, the name, and a typed value.
fn parse_storage(bytes: &[u8]) -> Option<Vec<(String, Value)>> {
    const VT_I4: u16 = 3;
    const VT_BSTR: u16 = 8;
    const VT_UI4: u16 = 0x13;
    const VT_LPWSTR: u16 = 0x1f;
    const VT_STREAM: u16 = 0x42;
    let utf16 = |b: &[u8]| {
        String::from_utf16_lossy(
            &b.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0)
                .collect::<Vec<_>>(),
        )
    };
    let sig = bytes.windows(4).position(|w| w == b"1SPS")?;
    let start = sig.checked_sub(4)?;
    let end = (start + u32_at(bytes, start)? as usize).min(bytes.len());
    let mut at = sig + 4 + 16;
    let mut values = Vec::new();
    while at + 9 <= end {
        let size = u32_at(bytes, at)? as usize;
        if size == 0 {
            break;
        }
        let name_size = u32_at(bytes, at + 4)? as usize;
        let name = utf16(bytes.get(at + 9..at + 9 + name_size)?);
        let v = at + 9 + name_size;
        let vt = u16::from_le_bytes([*bytes.get(v)?, *bytes.get(v + 1)?]);
        let value = match vt {
            VT_I4 | VT_UI4 => Some(Value::Number(u32_at(bytes, v + 4)?)),
            VT_BSTR | VT_LPWSTR => {
                let len = u32_at(bytes, v + 4)? as usize;
                let len = if vt == VT_LPWSTR { len * 2 } else { len };
                Some(Value::Text(utf16(bytes.get(v + 8..v + 8 + len)?)))
            }
            VT_STREAM => {
                // The stream's name ("prop4294967295"), padded to 4 bytes, then its size and data.
                let name_len = u32_at(bytes, v + 4)? as usize;
                let data = v + (8 + name_len).next_multiple_of(4);
                let len = u32_at(bytes, data)? as usize;
                Some(Value::Bytes(bytes.get(data + 4..data + 4 + len)?.to_vec()))
            }
            _ => None,
        };
        if let Some(value) = value {
            values.push((name, value));
        }
        at += size;
    }
    Some(values)
}

/// Writes a folder's view to its bag, as Explorer does when it leaves a folder whose view
/// changed: the mode (with the classic mode and view id Explorer keeps beside it), the sort,
/// the grouping and the columns.
pub fn write(path: &str, view: &ViewState) {
    com_init();
    let Ok(pidl) = Pidl::parse(path) else { return };
    let kind = folder_type(&pidl);
    let virtual_generic = kind == GENERIC
        && pidl.name(SIGDN_FILESYSPATH).is_empty()
        && !pidl
            .name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING)
            .eq_ignore_ascii_case("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}");
    let legacy = streamstore::Store::open(&pidl, virtual_generic);
    let bag_kind = persistence_type(&pidl, kind);
    let bag_name = wide(&format!(r"Shell\{{{bag_kind:?}}}"));
    let bag: Option<IPropertyBag> = if legacy.is_none() || virtual_generic {
        unsafe {
            SHGetViewStatePropertyBag(
                Some(pidl.0),
                PCWSTR(bag_name.as_ptr()),
                SHGVSPB_FOLDERNODEFAULTS,
            )
            .ok()
        }
    } else {
        None
    };
    if legacy.is_none() && bag.is_none() {
        return;
    }
    let put = |name: &str, value: VARIANT| {
        if let Some(store) = &legacy {
            store.put(name, &value);
        }
        if let Some(bag) = &bag {
            // A stream must be rewound after the legacy encoder consumed it.
            if let Some(stream) = variant_stream(&value) {
                unsafe {
                    let _ = stream.Seek(0, windows::Win32::System::Com::STREAM_SEEK_SET, None);
                }
            }
            let w = wide(name);
            unsafe {
                let _ = bag.Write(PCWSTR(w.as_ptr()), &value);
            }
        }
    };
    let stream = |bytes: Vec<u8>| {
        unsafe { SHCreateMemStream(Some(&bytes)) }
            .and_then(|s| s.cast::<IUnknown>().ok())
            .map(VARIANT::from)
    };
    // A bag Explorer never wrote gets the flags and revision it writes.
    if bag
        .as_ref()
        .is_some_and(|b| read_u32(b, "FFlags").is_none())
    {
        // Initialize only the bag; a legacy stream retains its own flag set.
        if let Some(bag) = &bag {
            unsafe {
                let _ = bag.Write(windows::core::w!("Rev"), &VARIANT::from(0u32));
                let flags = match bag_kind {
                    k if k == GUID::from_u128(0xd6d9e004_cd87_442b_9d57_5e0aeb4f6f72) => {
                        0x0120_0011u32
                    }
                    k if k == windows::Win32::UI::Shell::FOLDERTYPEID_NetworkExplorer => {
                        0x0120_0001u32
                    }
                    _ => 0x4120_0001u32,
                };
                let _ = bag.Write(windows::core::w!("FFlags"), &VARIANT::from(flags));
            }
        }
    }
    if let (Some(mode), Some(size)) = (view.logical_mode, view.icon_size) {
        // Measured: what Explorer writes for each of its eight modes.
        let (classic, vid) = match mode {
            1 => (4, "{137E7700-3573-11CF-AE69-08002B2E1262}"),
            2 => (6, "{65F125E5-7BE1-4810-BA9D-D271C8432CE3}"),
            4 => (3, "{0E1FA5E0-3573-11CF-AE69-08002B2E1262}"),
            5 => (8, "{30C2C434-0889-4C8D-985D-A9F71830B0A9}"),
            // Up to 31 the sizes are small icons (not measured past 16).
            _ if size < 32 => (1, "{089000C0-3573-11CF-AE69-08002B2E1262}"),
            _ => (1, "{0057D0E0-3573-11CF-AE69-08002B2E1262}"),
        };
        put("Vid", VARIANT::from(vid));
        put("Mode", VARIANT::from(classic as u32));
        put("LogicalViewMode", VARIANT::from(mode));
        put("IconSize", VARIANT::from(size));
    }
    if !view.sort.is_empty() {
        let mut bytes = (view.sort.len().min(16) as u32).to_le_bytes().to_vec();
        for (key, descending) in view.sort.iter().take(16) {
            push_key(&mut bytes, key);
            bytes.extend_from_slice(&(if *descending { -1i32 } else { 1 }).to_le_bytes());
        }
        if let Some(v) = stream(bytes) {
            put("Sort", v);
        }
    }
    if let Some(group) = view.group {
        let (key, descending) = group.unwrap_or_default();
        put(
            "GroupView",
            VARIANT::from(if group.is_some() { -1i32 } else { 0 }),
        );
        put(
            "GroupByKey:FMTID",
            VARIANT::from(format!("{{{:?}}}", key.fmtid).as_str()),
        );
        put("GroupByKey:PID", VARIANT::from(key.pid));
        put(
            "GroupByDirection",
            VARIANT::from(if descending { -1i32 } else { 1 }),
        );
    }
    if !view.columns.is_empty() {
        // The header: a signature, 16, two zeros, the count, then 24 (an entry's size).
        let mut bytes = Vec::new();
        for n in [0xfddf_dffdu32, 0x10, 0, 0, view.columns.len() as u32, 0x18] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        for (key, width) in &view.columns {
            push_key(&mut bytes, key);
            bytes.extend_from_slice(&width.to_le_bytes());
        }
        if let Some(v) = stream(bytes) {
            put("ColInfo", v);
        }
    }
    if let Some(store) = legacy {
        let _ = store.commit();
    }
}

/// Native namespace types absent from HKLM FolderTypes, measured by changing each
/// mode and diffing its bag. Keep legacy streams in sync for older shell views too.
fn persistence_type(pidl: &Pidl, kind: GUID) -> GUID {
    match pidl
        .name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING)
        .to_ascii_uppercase()
        .as_str()
    {
        "::{645FF040-5081-101B-9F08-00AA002F954E}" => {
            GUID::from_u128(0xd6d9e004_cd87_442b_9d57_5e0aeb4f6f72)
        }
        "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}" => {
            windows::Win32::UI::Shell::FOLDERTYPEID_NetworkExplorer
        }
        _ => kind,
    }
}
fn variant_stream(v: &VARIANT) -> Option<IStream> {
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        if inner.vt != VT_UNKNOWN {
            return None;
        }
        (*inner.Anonymous.punkVal).as_ref()?.cast().ok()
    }
}

/// A PROPERTYKEY as the bags store it: the GUID's fields little-endian, then the id.
fn push_key(bytes: &mut Vec<u8>, key: &PROPERTYKEY) {
    let g = key.fmtid;
    bytes.extend_from_slice(&g.data1.to_le_bytes());
    bytes.extend_from_slice(&g.data2.to_le_bytes());
    bytes.extend_from_slice(&g.data3.to_le_bytes());
    bytes.extend_from_slice(&g.data4);
    bytes.extend_from_slice(&key.pid.to_le_bytes());
}

/// The folder type: the one Explorer settled on for the folder (it names its bag), else a
/// known folder's own, else generic.
pub fn folder_type(pidl: &Pidl) -> GUID {
    if super::archive::root(&pidl.name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING))
        .is_some()
    {
        return windows::Win32::UI::Shell::FOLDERTYPEID_CompressedFolder;
    }
    unsafe {
        let bag: Option<IPropertyBag> =
            SHGetViewStatePropertyBag(Some(pidl.0), windows::core::w!("Shell"), SHGVSPB_FOLDER)
                .ok();
        if let Some(bag) = bag {
            // A known folder's type (Downloads), or the one sniffed from the contents.
            let derived = read_string(&bag, "KnownFolderDerivedFolderType")
                .and_then(|g| GUID::try_from(g.trim_matches(|c| c == '{' || c == '}')).ok());
            if let Some(kind) = derived.or_else(|| {
                read_string(&bag, "SniffedFolderType").and_then(|name| type_by_name(&name))
            }) {
                return kind;
            }
        }
        let path = pidl.name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING);
        if super::library::is_library(&path) {
            let canonical = super::library::folder_type(&path)
                .and_then(|kind| {
                    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
                        .open_subkey(format!(r"{}\{{{kind:?}}}", FOLDER_TYPES))
                        .ok()?
                        .get_value::<String, _>("CanonicalName")
                        .ok()
                })
                .unwrap_or_else(|| "Generic".into());
            if let Some(kind) = type_by_name(&format!("{canonical}.Library")) {
                return kind;
            }
        }
        let manager: windows::core::Result<IKnownFolderManager> =
            CoCreateInstance(&KnownFolderManager, None, CLSCTX_INPROC_SERVER);
        if let Ok(folder) = manager.and_then(|m| m.FindFolderFromIDList(pidl.0)) {
            let mut def = windows::Win32::UI::Shell::KNOWNFOLDER_DEFINITION::default();
            if folder.GetFolderDefinition(&mut def).is_ok() {
                let kind = def.ftidType;
                // FreeKnownFolderDefinitionFields is an inline helper, not an export.
                use windows::Win32::System::Com::CoTaskMemFree;
                for p in [
                    def.pszName,
                    def.pszDescription,
                    def.pszRelativePath,
                    def.pszParsingName,
                    def.pszTooltip,
                    def.pszLocalizedName,
                    def.pszIcon,
                    def.pszSecurity,
                ] {
                    CoTaskMemFree(Some(p.0 as *const _));
                }
                if kind != GUID::zeroed() {
                    return kind;
                }
            }
        }
    }
    // The virtual Libraries container isn't the filesystem Libraries known folder:
    // FindFolderFromIDList fails for its PIDL. Explorer uses the UsersLibraries type.
    if pidl
        .name(windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING)
        .eq_ignore_ascii_case("::{031E4825-7B94-4DC3-B131-E946B44C8DD5}")
    {
        return type_by_name("UsersLibraries").unwrap_or(GENERIC);
    }
    GENERIC
}

/// A folder's own columns (IShellFolder2), as Explorer lists them for a folder without a
/// type's list: shown are those on by default, and its header menu offers the others that
/// aren't secondary (the Recycle Bin: Nome, Local Original, Data de Exclusão, Tamanho, Tipo de
/// item, Data de modificação, then Data da criação, Atributos, Proprietário, Marcas...). With
/// each one's width in characters and header text.
fn folder_columns(pidl: &Pidl) -> Vec<(PROPERTYKEY, bool, Option<u32>, String, bool)> {
    const ONBYDEFAULT: u32 = 0x10;
    const SECONDARYUI: u32 = 0x80;
    const HIDDEN: u32 = 0x100;
    let Ok(folder) = pidl
        .folder()
        .and_then(|f| f.cast::<IShellFolder2>().map_err(|e| e.message()))
    else {
        return Vec::new();
    };
    let mut columns = Vec::new();
    unsafe {
        for i in 0.. {
            let Ok(state) = folder.GetDefaultColumnState(i) else {
                break;
            };
            let state = state.0 as u32;
            if state & (SECONDARYUI | HIDDEN) != 0 {
                continue;
            }
            let mut key = PROPERTYKEY::default();
            if folder.MapColumnToSCID(i, &mut key).is_err() {
                continue;
            }
            let mut details = SHELLDETAILS::default();
            let (mut chars, mut label, mut right) = (None, String::new(), false);
            if folder
                .GetDetailsOf(std::ptr::null(), i, &mut details)
                .is_ok()
            {
                chars = Some(details.cxChar as u32).filter(|&c| c > 0);
                // LVCFMT_RIGHT.
                right = details.fmt & 1 != 0;
                let mut text = details.str;
                let mut out = windows::core::PWSTR::null();
                if StrRetToStrW(&mut text, None, &mut out).is_ok() {
                    label = super::take_pwstr(out);
                }
            }
            columns.push((key, state & ONBYDEFAULT != 0, chars, label, right));
        }
    }
    columns
}

/// A folder type by its canonical name ("Downloads", "Pictures"...).
fn type_by_name(name: &str) -> Option<GUID> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let types = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(FOLDER_TYPES)
        .ok()?;
    types.enum_keys().flatten().find_map(|key| {
        let canonical: String = types
            .open_subkey(&key)
            .ok()?
            .get_value("CanonicalName")
            .ok()?;
        (canonical.eq_ignore_ascii_case(name))
            .then(|| GUID::try_from(key.trim_matches(|c| c == '{' || c == '}')).ok())
            .flatten()
    })
}

/// The view of search results in a folder: the defaults of its type's results type
/// (Generic.SearchResults for a generic folder: content view, by rank, then date modified).
pub fn read_search(scope: &str) -> ViewState {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    com_init();
    let kind = Pidl::parse(scope)
        .map(|p| folder_type(&p))
        .unwrap_or(GENERIC);
    let types = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(FOLDER_TYPES)
        .ok();
    let name: Option<String> = types
        .as_ref()
        .and_then(|t| t.open_subkey(format!("{{{kind:?}}}")).ok())
        .and_then(|k| k.get_value("CanonicalName").ok());
    let results = name
        .and_then(|n| type_by_name(&format!("{n}.SearchResults")))
        .or_else(|| type_by_name("Generic.SearchResults"))
        .unwrap_or(GENERIC);
    let mut state = defaults(&results);
    // Search ColumnList has no explicit character counts. Its widths come from each
    // property description, not the ordinary folder's wider 272px Name default.
    state.columns = state
        .type_columns
        .iter()
        .filter(|(_, shown, _)| *shown)
        .filter_map(|(key, _, chars)| {
            let width = chars
                .map(|c| c * 8)
                .or_else(|| super::columns::describe(key).map(|c| c.width as u32))?;
            Some((*key, width))
        })
        .collect();
    state
}

/// The folder type's default view: its "TopViews" entry {0…0}, else the one marked
/// PrimarySettings (the search results types'), else the first by Order.
fn defaults(kind: &GUID) -> ViewState {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let mut state = ViewState::default();
    let Ok(top) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(format!(r"{FOLDER_TYPES}\{{{kind:?}}}\TopViews"))
    else {
        return state;
    };
    let views: Vec<(String, winreg::RegKey)> = top
        .enum_keys()
        .flatten()
        .filter_map(|n| top.open_subkey(&n).ok().map(|k| (n, k)))
        .collect();
    let primary = views
        .iter()
        .find(|(n, _)| n == "{00000000-0000-0000-0000-000000000000}")
        .or_else(|| {
            views
                .iter()
                .find(|(_, k)| k.get_value::<u32, _>("PrimarySettings").unwrap_or(0) == 1)
        })
        .or_else(|| {
            views
                .iter()
                .min_by_key(|(_, k)| k.get_value::<u32, _>("Order").unwrap_or(u32::MAX))
        });
    let Some((_, view)) = primary else {
        return state;
    };
    state.logical_mode = view.get_value::<u32, _>("LogicalViewMode").ok();
    state.icon_size = view.get_value::<u32, _>("IconSize").ok();
    let group: String = view.get_value("GroupBy").unwrap_or_default();
    if let Some(key) = property_key(group.trim()) {
        let ascending = view.get_value::<u32, _>("GroupAscending").unwrap_or(1) != 0;
        state.group = Some(Some((key, !ascending)));
    }
    let columns: String = view.get_value("ColumnList").unwrap_or_default();
    state.type_columns = columns
        .trim_start_matches("prop:")
        .split(';')
        .filter_map(type_column)
        .collect();
    let sort: String = view.get_value("SortByList").unwrap_or_default();
    state.sort = sort
        .trim_start_matches("prop:")
        .split(';')
        .filter_map(|p| {
            let p = p.trim();
            let (descending, name) = match p.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, p.strip_prefix('+').unwrap_or(p)),
            };
            property_key(name).map(|k| (k, descending))
        })
        .collect();
    state
}

/// A "ColumnList" entry: "0(34)System.ItemNameDisplay" is shown (0, else 1), 34 characters
/// wide.
fn type_column(entry: &str) -> Option<(PROPERTYKEY, bool, Option<u32>)> {
    let entry = entry.trim();
    let shown = !entry.starts_with('1');
    let rest = entry.trim_start_matches(|c: char| c.is_ascii_digit());
    let (width, name) = match rest.strip_prefix('(').and_then(|r| r.split_once(')')) {
        Some((w, name)) => (w.parse().ok(), name),
        None => (None, rest),
    };
    Some((property_key(name)?, shown, width))
}

fn property_key(name: &str) -> Option<PROPERTYKEY> {
    if name.is_empty() {
        return None;
    }
    let w = wide(name);
    let mut key = PROPERTYKEY::default();
    unsafe { PSGetPropertyKeyFromName(PCWSTR(w.as_ptr()), &mut key).ok()? };
    Some(key)
}

fn value(bag: &IPropertyBag, name: &str) -> Option<VARIANT> {
    let w = wide(name);
    let mut value = VARIANT::default();
    unsafe { bag.Read(PCWSTR(w.as_ptr()), &mut value, None).ok()? };
    Some(value)
}

fn read_u32(bag: &IPropertyBag, name: &str) -> Option<u32> {
    let v = value(bag, name)?;
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        match inner.vt {
            VT_UI4 => Some(inner.Anonymous.ulVal),
            VT_I4 => Some(inner.Anonymous.lVal as u32),
            _ => None,
        }
    }
}

fn read_string(bag: &IPropertyBag, name: &str) -> Option<String> {
    let v = value(bag, name)?;
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        // Registry strings come with their terminator, and sometimes bytes past it.
        (inner.vt == VT_BSTR).then(|| {
            inner
                .Anonymous
                .bstrVal
                .to_string()
                .split(' ')
                .next()
                .unwrap_or_default()
                .to_string()
        })
    }
}

fn read_stream(bag: &IPropertyBag, name: &str) -> Option<Vec<u8>> {
    let v = value(bag, name)?;
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        if inner.vt != VT_UNKNOWN {
            return None;
        }
        let stream: IStream = (*inner.Anonymous.punkVal).as_ref()?.cast().ok()?;
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let mut read = 0u32;
            if stream
                .Read(
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    Some(&mut read),
                )
                .is_err()
                || read == 0
            {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        Some(out)
    }
}

fn key_at(bytes: &[u8], at: usize) -> Option<PROPERTYKEY> {
    let fmtid = GUID::from_u128(u128::from_be_bytes({
        // GUIDs are stored as Data1 (LE u32), Data2, Data3 (LE u16), Data4 (8 bytes).
        let b = bytes.get(at..at + 16)?;
        let mut g = [0u8; 16];
        g[0..4].copy_from_slice(&[b[3], b[2], b[1], b[0]]);
        g[4..6].copy_from_slice(&[b[5], b[4]]);
        g[6..8].copy_from_slice(&[b[7], b[6]]);
        g[8..16].copy_from_slice(&b[8..16]);
        g
    }));
    Some(PROPERTYKEY {
        fmtid,
        pid: u32_at(bytes, at + 16)?,
    })
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// "Sort": a count, then per column its PROPERTYKEY and direction (1 or -1).
fn parse_sort(bytes: &[u8]) -> Vec<(PROPERTYKEY, bool)> {
    let count = u32_at(bytes, 0).unwrap_or(0) as usize;
    (0..count.min(16))
        .filter_map(|i| {
            let at = 4 + i * 24;
            Some((key_at(bytes, at)?, u32_at(bytes, at + 20)? as i32 == -1))
        })
        // An empty key is no sort (no arrow, Explorer lists in name order).
        .filter(|(k, _)| k.fmtid != GUID::zeroed())
        .collect()
}

/// "ColInfo": a 24-byte header (with the count at 16), then per column its PROPERTYKEY and
/// width.
fn parse_columns(bytes: &[u8]) -> Vec<(PROPERTYKEY, u32)> {
    if u32_at(bytes, 0) != Some(0xfddf_dffd) {
        return Vec::new();
    }
    let count = u32_at(bytes, 16).unwrap_or(0) as usize;
    (0..count.min(64))
        .filter_map(|i| {
            let at = 24 + i * 24;
            Some((key_at(bytes, at)?, u32_at(bytes, at + 20)?))
        })
        .collect()
}

#[cfg(test)]
mod type_default_tests {
    use super::*;

    #[test]
    fn native_type_default_then_individual_folder_override() {
        const GENERIC_DEFAULT_96: [u8; 772] = [
            0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf1, 0xf1, 0xf1, 0xf1,
            0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xd0, 0x02, 0x00, 0x00, 0xcc, 0x02, 0x00, 0x00, 0x31, 0x53, 0x50, 0x53, 0x05, 0xd5, 0xcd, 0xd5,
            0x9c, 0x2e, 0x1b, 0x10, 0x93, 0x97, 0x08, 0x00, 0x2b, 0x2c, 0xf9, 0xae, 0x83, 0x00, 0x00, 0x00,
            0x22, 0x00, 0x00, 0x00, 0x00, 0x47, 0x00, 0x72, 0x00, 0x6f, 0x00, 0x75, 0x00, 0x70, 0x00, 0x42,
            0x00, 0x79, 0x00, 0x4b, 0x00, 0x65, 0x00, 0x79, 0x00, 0x3a, 0x00, 0x46, 0x00, 0x4d, 0x00, 0x54,
            0x00, 0x49, 0x00, 0x44, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x4e, 0x00, 0x00, 0x00, 0x7b,
            0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30,
            0x00, 0x2d, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x2d, 0x00, 0x30, 0x00, 0x30,
            0x00, 0x30, 0x00, 0x30, 0x00, 0x2d, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x2d,
            0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30,
            0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x7d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x33,
            0x00, 0x00, 0x00, 0x22, 0x00, 0x00, 0x00, 0x00, 0x47, 0x00, 0x72, 0x00, 0x6f, 0x00, 0x75, 0x00,
            0x70, 0x00, 0x42, 0x00, 0x79, 0x00, 0x44, 0x00, 0x69, 0x00, 0x72, 0x00, 0x65, 0x00, 0x63, 0x00,
            0x74, 0x00, 0x69, 0x00, 0x6f, 0x00, 0x6e, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x01, 0x00,
            0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x53, 0x00, 0x6f, 0x00, 0x72,
            0x00, 0x74, 0x00, 0x00, 0x00, 0x42, 0x00, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x70, 0x00, 0x72,
            0x00, 0x6f, 0x00, 0x70, 0x00, 0x34, 0x00, 0x32, 0x00, 0x39, 0x00, 0x34, 0x00, 0x39, 0x00, 0x36,
            0x00, 0x37, 0x00, 0x32, 0x00, 0x39, 0x00, 0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x00, 0x00,
            0x00, 0x01, 0x00, 0x00, 0x00, 0x30, 0xf1, 0x25, 0xb7, 0xef, 0x47, 0x1a, 0x10, 0xa5, 0xf1, 0x02,
            0x60, 0x8c, 0x9e, 0xeb, 0xac, 0x0a, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x25, 0x00, 0x00,
            0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x47, 0x00, 0x72, 0x00, 0x6f, 0x00, 0x75, 0x00, 0x70, 0x00,
            0x56, 0x00, 0x69, 0x00, 0x65, 0x00, 0x77, 0x00, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x1b, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x4d, 0x00, 0x6f, 0x00, 0x64,
            0x00, 0x65, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x23, 0x00, 0x00,
            0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x49, 0x00, 0x63, 0x00, 0x6f, 0x00, 0x6e, 0x00, 0x53, 0x00,
            0x69, 0x00, 0x7a, 0x00, 0x65, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x60, 0x00, 0x00, 0x00,
            0xbd, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x43, 0x00, 0x6f, 0x00, 0x6c, 0x00, 0x49,
            0x00, 0x6e, 0x00, 0x66, 0x00, 0x6f, 0x00, 0x00, 0x00, 0x42, 0x00, 0x00, 0x00, 0x1e, 0x00, 0x00,
            0x00, 0x70, 0x00, 0x72, 0x00, 0x6f, 0x00, 0x70, 0x00, 0x34, 0x00, 0x32, 0x00, 0x39, 0x00, 0x34,
            0x00, 0x39, 0x00, 0x36, 0x00, 0x37, 0x00, 0x32, 0x00, 0x39, 0x00, 0x35, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x78, 0x00, 0x00, 0x00, 0xfd, 0xdf, 0xdf, 0xfd, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x30, 0xf1, 0x25,
            0xb7, 0xef, 0x47, 0x1a, 0x10, 0xa5, 0xf1, 0x02, 0x60, 0x8c, 0x9e, 0xeb, 0xac, 0x0a, 0x00, 0x00,
            0x00, 0x10, 0x01, 0x00, 0x00, 0x30, 0xf1, 0x25, 0xb7, 0xef, 0x47, 0x1a, 0x10, 0xa5, 0xf1, 0x02,
            0x60, 0x8c, 0x9e, 0xeb, 0xac, 0x0e, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x30, 0xf1, 0x25,
            0xb7, 0xef, 0x47, 0x1a, 0x10, 0xa5, 0xf1, 0x02, 0x60, 0x8c, 0x9e, 0xeb, 0xac, 0x04, 0x00, 0x00,
            0x00, 0x78, 0x00, 0x00, 0x00, 0x30, 0xf1, 0x25, 0xb7, 0xef, 0x47, 0x1a, 0x10, 0xa5, 0xf1, 0x02,
            0x60, 0x8c, 0x9e, 0xeb, 0xac, 0x0c, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x2f, 0x00, 0x00,
            0x00, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x47, 0x00, 0x72, 0x00, 0x6f, 0x00, 0x75, 0x00, 0x70, 0x00,
            0x42, 0x00, 0x79, 0x00, 0x4b, 0x00, 0x65, 0x00, 0x79, 0x00, 0x3a, 0x00, 0x50, 0x00, 0x49, 0x00,
            0x44, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00,
            0x0e, 0x00, 0x00, 0x00, 0x00, 0x46, 0x00, 0x46, 0x00, 0x6c, 0x00, 0x61, 0x00, 0x67, 0x00, 0x73,
            0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x01, 0x00, 0x20, 0x41, 0x31, 0x00, 0x00, 0x00, 0x20,
            0x00, 0x00, 0x00, 0x00, 0x4c, 0x00, 0x6f, 0x00, 0x67, 0x00, 0x69, 0x00, 0x63, 0x00, 0x61, 0x00,
            0x6c, 0x00, 0x56, 0x00, 0x69, 0x00, 0x65, 0x00, 0x77, 0x00, 0x4d, 0x00, 0x6f, 0x00, 0x64, 0x00,
            0x65, 0x00, 0x00, 0x00, 0x13, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let saved = Saved::Stream(parse_storage(&GENERIC_DEFAULT_96).expect("native serialized default"));
        let mut state = ViewState {
            logical_mode: Some(1),
            icon_size: Some(16),
            ..Default::default()
        };
        apply_saved(&saved, &mut state);
        assert_eq!(state.logical_mode, Some(3));
        assert_eq!(state.icon_size, Some(96));
        assert_eq!(state.sort.len(), 1);
        assert_eq!(state.sort[0].0.pid, 10);
        assert!(!state.sort[0].1);
        assert_eq!(state.columns.len(), 4);
        assert_eq!(state.group, Some(None));
        let columns = state.columns.clone();
        apply_saved(
            &Saved::Stream(vec![
                ("LogicalViewMode".into(), Value::Number(1)),
                ("IconSize".into(), Value::Number(16)),
            ]),
            &mut state,
        );
        assert_eq!((state.logical_mode, state.icon_size), (Some(1), Some(16)));
        assert_eq!(state.columns, columns);
    }
}
