//! "Escolher Detalhes", the header menu's "Mais...": shell32's own dialog (resource 1096,
//! so its layout and texts are the real's), filled and run as Explorer does:
//! - the shown columns first, checked and in order, then every other viewable property by
//!   name;
//! - moving an item up or down (even above Nome), showing or hiding it (Nome can't be
//!   hidden);
//! - the selected column's width, 0 for one that isn't shown yet.

use std::cmp::Ordering;

use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Globalization::{CompareStringEx, COMPARESTRING_RESULT, LINGUISTIC_IGNORECASE};
use windows::Win32::Storage::EnhancedStorage::PKEY_ItemNameDisplay;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    LVCOLUMNW, LVCF_WIDTH, LVIF_STATE, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVIS_STATEIMAGEMASK, LVITEMW, LVM_ENSUREVISIBLE,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVN_ITEMCHANGED,
    LVN_ITEMCHANGING, LVSCW_AUTOSIZE, LVS_EX_CHECKBOXES, NMHDR, NMLISTVIEW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyDescription, IPropertyDescriptionList, PSEnumeratePropertyDescriptions, PDEF_VIEWABLE};
use windows::Win32::UI::WindowsAndMessaging::{
    DialogBoxParamW, EndDialog, GetDlgItem, GetDlgItemInt, GetWindowLongPtrW, SendMessageW, SetDlgItemInt, SetWindowLongPtrW, EN_CHANGE, GWLP_USERDATA, IDCANCEL,
    IDOK, WINDOW_LONG_PTR_INDEX, WM_COMMAND, WM_INITDIALOG, WM_NOTIFY,
};

use crate::shell::wide;

const DIALOG: u16 = 1096;
const LIST: i32 = 1001;
const UP: i32 = 1003;
const DOWN: i32 = 1004;
const WIDTH: i32 = 1005;
const SHOW: i32 = 1006;
const HIDE: i32 = 1007;
/// The dialog procedure's result for a notification (DWLP_MSGRESULT).
const MSGRESULT: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(0);

struct Item {
    key: PROPERTYKEY,
    label: String,
    checked: bool,
    /// Pixels at 100%: the shown columns' widths and those typed in; None until shown.
    width: Option<u32>,
}

struct Dialog {
    items: Vec<Item>,
    selected: usize,
    /// Set while the dialog changes the list or the width itself.
    quiet: bool,
}

/// Runs the dialog over the shown columns; on OK, the columns to show, in order, with the
/// widths known (None for those just added).
pub fn choose(owner: HWND, shown: &[(PROPERTYKEY, f32)]) -> Option<Vec<(PROPERTYKEY, Option<u32>)>> {
    let mut items: Vec<Item> = shown
        .iter()
        .filter_map(|(key, w)| super::columns::label(key).map(|label| Item { key: *key, label, checked: true, width: Some(w.round() as u32) }))
        .collect();
    let mut rest: Vec<Item> = viewable()
        .into_iter()
        .filter(|(key, _)| !shown.iter().any(|(k, _)| k == key))
        .map(|(key, label)| Item { key, label, checked: false, width: None })
        .collect();
    rest.sort_by(|a, b| compare(&a.label, &b.label));
    items.extend(rest);
    let mut dialog = Dialog { items, selected: 0, quiet: false };
    let ok = unsafe {
        let shell32 = GetModuleHandleW(windows::core::w!("shell32.dll")).ok()?;
        DialogBoxParamW(Some(HINSTANCE(shell32.0)), PCWSTR(DIALOG as usize as *const u16), Some(owner), Some(proc), LPARAM(&mut dialog as *mut Dialog as isize))
    };
    (ok == IDOK.0 as isize).then(|| dialog.items.iter().filter(|i| i.checked).map(|i| (i.key, i.width)).collect())
}

/// The property system's viewable properties that have a name.
fn viewable() -> Vec<(PROPERTYKEY, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut list: Option<IPropertyDescriptionList> = None;
        if PSEnumeratePropertyDescriptions(PDEF_VIEWABLE, &IPropertyDescriptionList::IID, &mut list as *mut _ as *mut _).is_err() {
            return out;
        }
        let Some(list) = list else { return out };
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(desc) = list.GetAt::<IPropertyDescription>(i) else { continue };
            let label = desc.GetDisplayName().map(|p| crate::shell::take_pwstr(p)).unwrap_or_default();
            let mut key = PROPERTYKEY::default();
            if !label.is_empty() && desc.GetPropertyKey(&mut key).is_ok() {
                out.push((key, label));
            }
        }
    }
    out
}

/// Names in the user's language order, ignoring case and accents' weight.
fn compare(a: &str, b: &str) -> Ordering {
    let (a, b): (Vec<u16>, Vec<u16>) = (a.encode_utf16().collect(), b.encode_utf16().collect());
    match unsafe { CompareStringEx(PCWSTR::null(), LINGUISTIC_IGNORECASE, &a, &b, None, None, None) } {
        COMPARESTRING_RESULT(1) => Ordering::Less,
        COMPARESTRING_RESULT(3) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn item(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id).unwrap_or_default() }
}

fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(w)), Some(LPARAM(l))).0 }
}

/// A row's check box: state image 2 is checked, 1 unchecked.
fn set_check(list: HWND, row: usize, checked: bool) {
    let lv = LVITEMW { stateMask: LVIS_STATEIMAGEMASK, state: windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(if checked { 2 << 12 } else { 1 << 12 }), ..Default::default() };
    send(list, LVM_SETITEMSTATE, row, &lv as *const _ as isize);
}

fn set_text(list: HWND, row: usize, text: &str) {
    let mut w = wide(text);
    let lv = LVITEMW { pszText: PWSTR(w.as_mut_ptr()), ..Default::default() };
    send(list, LVM_SETITEMTEXTW, row, &lv as *const _ as isize);
}

fn select(list: HWND, row: usize) {
    let both = windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
    let lv = LVITEMW { stateMask: both, state: both, ..Default::default() };
    send(list, LVM_SETITEMSTATE, row, &lv as *const _ as isize);
    send(list, LVM_ENSUREVISIBLE, row, 0);
}

impl Dialog {
    fn init(&mut self, hwnd: HWND) {
        let list = item(hwnd, LIST);
        send(list, LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_CHECKBOXES as usize, LVS_EX_CHECKBOXES as isize);
        let column = LVCOLUMNW { mask: LVCF_WIDTH, cx: 100, ..Default::default() };
        send(list, LVM_INSERTCOLUMNW, 0, &column as *const _ as isize);
        self.quiet = true;
        for (row, it) in self.items.iter().enumerate() {
            let mut w = wide(&it.label);
            let lv = LVITEMW { mask: LVIF_TEXT, iItem: row as i32, pszText: PWSTR(w.as_mut_ptr()), ..Default::default() };
            send(list, LVM_INSERTITEMW, 0, &lv as *const _ as isize);
            set_check(list, row, it.checked);
        }
        send(list, LVM_SETCOLUMNWIDTH, 0, LVSCW_AUTOSIZE as isize);
        select(list, 0);
        self.quiet = false;
        self.selected = 0;
        self.update(hwnd);
        unsafe {
            let _ = SetFocus(Some(list));
        }
    }

    /// The buttons and the width for the selected row.
    fn update(&mut self, hwnd: HWND) {
        let n = self.items.len();
        let row = self.selected;
        let Some(it) = self.items.get(row) else { return };
        let name = it.key == PKEY_ItemNameDisplay;
        unsafe {
            let _ = EnableWindow(item(hwnd, UP), row > 0);
            let _ = EnableWindow(item(hwnd, DOWN), row + 1 < n);
            let _ = EnableWindow(item(hwnd, SHOW), !it.checked);
            let _ = EnableWindow(item(hwnd, HIDE), it.checked && !name);
            self.quiet = true;
            let _ = SetDlgItemInt(hwnd, WIDTH, if it.checked { it.width.unwrap_or(0) } else { 0 }, false);
            self.quiet = false;
        }
    }

    fn check(&mut self, hwnd: HWND, checked: bool) {
        let list = item(hwnd, LIST);
        set_check(list, self.selected, checked);
        // The button pressed is now disabled: the other one takes the focus.
        unsafe {
            let _ = SetFocus(Some(item(hwnd, if checked { HIDE } else { SHOW })));
        }
    }

    fn shift(&mut self, hwnd: HWND, down: bool) {
        let from = self.selected;
        let to = if down { from + 1 } else { from.wrapping_sub(1) };
        if to >= self.items.len() {
            return;
        }
        let list = item(hwnd, LIST);
        self.items.swap(from, to);
        self.quiet = true;
        for row in [from, to] {
            set_text(list, row, &self.items[row].label);
            set_check(list, row, self.items[row].checked);
        }
        self.quiet = false;
        select(list, to);
    }

    fn notify(&mut self, hwnd: HWND, nm: &NMLISTVIEW) -> Option<isize> {
        let row = usize::try_from(nm.iItem).ok()?;
        let image = |state: u32| (state & LVIS_STATEIMAGEMASK.0) >> 12;
        let changes_check = nm.uChanged.0 & LVIF_STATE.0 != 0 && image(nm.uNewState) != image(nm.uOldState) && image(nm.uOldState) != 0;
        match nm.hdr.code {
            // Nome stays checked.
            LVN_ITEMCHANGING if changes_check && image(nm.uNewState) == 1 && self.items.get(row)?.key == PKEY_ItemNameDisplay => Some(1),
            LVN_ITEMCHANGED if nm.uChanged.0 & LVIF_STATE.0 != 0 => {
                if changes_check {
                    self.items.get_mut(row)?.checked = image(nm.uNewState) == 2;
                }
                if nm.uNewState & LVIS_SELECTED.0 != 0 && nm.uOldState & LVIS_SELECTED.0 == 0 {
                    self.selected = row;
                }
                if !self.quiet && (changes_check || row == self.selected) {
                    self.update(hwnd);
                }
                None
            }
            _ => None,
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, lparam.0);
        let dialog = &mut *(lparam.0 as *mut Dialog);
        dialog.init(hwnd);
        // The focus was set on the list.
        return 0;
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
    if ptr.is_null() {
        return 0;
    }
    let dialog = &mut *ptr;
    match msg {
        WM_NOTIFY => {
            let hdr = &*(lparam.0 as *const NMHDR);
            if hdr.idFrom != LIST as usize || (hdr.code != LVN_ITEMCHANGING && hdr.code != LVN_ITEMCHANGED) {
                return 0;
            }
            match dialog.notify(hwnd, &*(lparam.0 as *const NMLISTVIEW)) {
                Some(result) => {
                    SetWindowLongPtrW(hwnd, MSGRESULT, result);
                    1
                }
                None => 0,
            }
        }
        WM_COMMAND => {
            let (id, code) = ((wparam.0 & 0xffff) as i32, (wparam.0 >> 16) as u32);
            match id {
                UP | DOWN => dialog.shift(hwnd, id == DOWN),
                SHOW | HIDE => dialog.check(hwnd, id == SHOW),
                WIDTH if code == EN_CHANGE && !dialog.quiet => {
                    let width = GetDlgItemInt(hwnd, WIDTH, None, false);
                    if let Some(it) = dialog.items.get_mut(dialog.selected) {
                        it.width = Some(width);
                    }
                }
                _ if id == IDOK.0 || id == IDCANCEL.0 => {
                    let _ = EndDialog(hwnd, id as isize);
                }
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}
