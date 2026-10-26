//! Explorer's legacy per-PIDL ViewView2 store. Patch named properties while retaining
//! its header, unrelated property entries, other storage sections and trailing bytes.
use super::{com_init, Pidl};
use std::cell::RefCell;
use windows::core::{Interface, GUID};
use windows::Win32::System::Com::IStream;
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VT_UI4, VT_UNKNOWN};
use winreg::{enums::*, RegKey, RegValue};

const ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer";
pub struct Store {
    key: RegKey,
    name: String,
    bytes: RefCell<Vec<u8>>,
}
impl Store {
    pub fn open(pidl: &Pidl, create: bool) -> Option<Self> {
        com_init();
        let user = RegKey::predef(HKEY_CURRENT_USER);
        let size = unsafe { windows::Win32::UI::Shell::ILGetSize(Some(pidl.0)) } as usize;
        let id = unsafe { std::slice::from_raw_parts(pidl.0 as *const u8, size) };
        let mru = if create {
            user.create_subkey(format!(r"{ROOT}\StreamMRU")).ok()?.0
        } else {
            user.open_subkey(format!(r"{ROOT}\StreamMRU")).ok()?
        };
        let found = mru
            .enum_values()
            .flatten()
            .find(|(_, v)| v.bytes == id)
            .map(|(n, _)| n);
        let name = match found {
            Some(name) => name,
            None if create => {
                let streams = user.create_subkey(format!(r"{ROOT}\Streams")).ok()?.0;
                let name = (0u32..)
                    .find(|n| {
                        mru.get_raw_value(&n.to_string()).is_err()
                            && streams.open_subkey(n.to_string()).is_err()
                    })?
                    .to_string();
                mru.set_raw_value(
                    &name,
                    &RegValue {
                        vtype: REG_BINARY,
                        bytes: id.to_vec(),
                    },
                )
                .ok()?;
                let mut order = mru
                    .get_raw_value("MRUListEx")
                    .map(|v| v.bytes)
                    .unwrap_or_else(|_| u32::MAX.to_le_bytes().to_vec());
                order.splice(0..0, name.parse::<u32>().ok()?.to_le_bytes());
                mru.set_raw_value(
                    "MRUListEx",
                    &RegValue {
                        vtype: REG_BINARY,
                        bytes: order,
                    },
                )
                .ok()?;
                name
            }
            _ => return None,
        };
        let path = format!(r"{ROOT}\Streams\{name}");
        let key = if create {
            user.create_subkey(path).ok()?.0
        } else {
            user.open_subkey(path).ok()?
        };
        let bytes = key
            .get_raw_value("ViewView2")
            .map(|v| v.bytes)
            .ok()
            .or_else(|| create.then(empty))?;
        section(&bytes)?;
        Some(Self {
            key,
            name,
            bytes: RefCell::new(bytes),
        })
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.borrow().clone()
    }
    pub fn put(&self, name: &str, value: &VARIANT) {
        // These two are used by bags, not by ViewView2.
        if matches!(name, "Vid" | "Rev") {
            return;
        }
        if let Some(mut data) = encode(value) {
            if name == "GroupView" {
                data[0..2].copy_from_slice(&11u16.to_le_bytes());
                let yes = data[4..].iter().any(|b| *b != 0);
                data[4..8].copy_from_slice(&(if yes { 65535u32 } else { 0 }).to_le_bytes());
            }
            let mut bytes = self.bytes.borrow_mut();
            if let Some(updated) = replace(&bytes, name, &data) {
                *bytes = updated;
            }
        }
    }
    pub fn commit(&self) -> std::io::Result<()> {
        self.key
            .open_subkey_with_flags("", KEY_SET_VALUE)?
            .set_raw_value(
                "ViewView2",
                &RegValue {
                    vtype: REG_BINARY,
                    bytes: self.bytes.borrow().clone(),
                },
            )?;
        let mru = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(format!(r"{ROOT}\StreamMRU"), KEY_READ | KEY_WRITE)?;
        if let (Ok(index), Ok(order)) = (self.name.parse::<u32>(), mru.get_raw_value("MRUListEx")) {
            let mut values = vec![index];
            values.extend(
                order
                    .bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .filter(|n| *n != index && *n != u32::MAX),
            );
            values.push(u32::MAX);
            mru.set_raw_value(
                "MRUListEx",
                &RegValue {
                    vtype: REG_BINARY,
                    bytes: values.into_iter().flat_map(u32::to_le_bytes).collect(),
                },
            )?;
        }
        Ok(())
    }
}
fn empty() -> Vec<u8> {
    // Reference header: 28-byte old view header, extension marker, 20-byte extension.
    let mut bytes = vec![0u8; 52];
    bytes[..4].copy_from_slice(&28u32.to_le_bytes());
    bytes[28..32].fill(0xf1);
    bytes[32..36].copy_from_slice(&20u32.to_le_bytes());
    bytes[49] = 3;
    bytes.extend_from_slice(&28u32.to_le_bytes());
    bytes.extend_from_slice(b"1SPS");
    let guid = GUID::from_u128(0xd5cdd505_2e9c_101b_9397_08002b2cf9ae);
    bytes.extend_from_slice(&guid.data1.to_le_bytes());
    bytes.extend_from_slice(&guid.data2.to_le_bytes());
    bytes.extend_from_slice(&guid.data3.to_le_bytes());
    bytes.extend_from_slice(&guid.data4);
    bytes.extend_from_slice(&[0; 8]);
    bytes
}
fn number(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
}
fn section(bytes: &[u8]) -> Option<(usize, usize)> {
    let at = bytes
        .windows(20)
        .position(|b| b == b"1SPS\x05\xd5\xcd\xd5\x9c\x2e\x1b\x10\x93\x97\x08\x00\x2b\x2c\xf9\xae")?
        .checked_sub(4)?;
    let end = at.checked_add(number(bytes, at)?)?;
    (end <= bytes.len() && end >= at + 28).then_some((at, end))
}
fn replace(bytes: &[u8], name: &str, data: &[u8]) -> Option<Vec<u8>> {
    let (start, end) = section(bytes)?;
    let mut at = start + 24;
    let mut replace_end = at;
    loop {
        let size = number(bytes, at)?;
        if size == 0 {
            break;
        }
        if size < 9 || at + size > end {
            return None;
        }
        let len = number(bytes, at + 4)?;
        if len % 2 != 0 || 9 + len > size {
            return None;
        }
        let units: Vec<_> = bytes
            .get(at + 9..at + 9 + len)?
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|c| *c != 0)
            .collect();
        if String::from_utf16(&units).ok()?.eq_ignore_ascii_case(name) {
            replace_end = at + size;
            break;
        }
        at += size;
        replace_end = at;
    }
    let name: Vec<u8> = name
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut entry = ((9 + name.len() + data.len()) as u32)
        .to_le_bytes()
        .to_vec();
    entry.extend_from_slice(&(name.len() as u32).to_le_bytes());
    entry.push(0);
    entry.extend(name);
    entry.extend(data);
    let mut out = bytes.to_vec();
    out.splice(at..replace_end, entry);
    let size = (end - start + out.len() - bytes.len()) as u32;
    out[start..start + 4].copy_from_slice(&size.to_le_bytes());
    Some(out)
}
fn encode(value: &VARIANT) -> Option<Vec<u8>> {
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        let mut out = inner.vt.0.to_le_bytes().to_vec();
        out.extend_from_slice(&[0, 0]);
        match inner.vt {
            VT_I4 => out.extend_from_slice(&inner.Anonymous.lVal.to_le_bytes()),
            VT_UI4 => out.extend_from_slice(&inner.Anonymous.ulVal.to_le_bytes()),
            VT_BSTR => {
                let text: Vec<u8> = inner
                    .Anonymous
                    .bstrVal
                    .to_string()
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect();
                out.extend_from_slice(&(text.len() as u32).to_le_bytes());
                out.extend(text)
            }
            VT_UNKNOWN => {
                let stream: IStream = (*inner.Anonymous.punkVal).as_ref()?.cast().ok()?;
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let mut n = 0;
                    if stream
                        .Read(buf.as_mut_ptr() as _, 4096, Some(&mut n))
                        .is_err()
                        || n == 0
                    {
                        break;
                    }
                    data.extend_from_slice(&buf[..n as usize]);
                }
                out[0..2].copy_from_slice(&66u16.to_le_bytes());
                let name: Vec<_> = "prop4294967295"
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect();
                out.extend_from_slice(&(name.len() as u32).to_le_bytes());
                out.extend(name);
                out.resize(out.len().next_multiple_of(4), 0);
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend(data);
            }
            _ => return None,
        };
        // Serialized PROPVARIANT payloads are padded to a DWORD boundary (relative
        // to the payload, not to the enclosing odd-sized named entry).
        out.resize(out.len().next_multiple_of(4), 0);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patch_preserves_header_unknown_properties_and_other_sections() {
        let mut original = empty();
        original.extend_from_slice(b"other storage section, retained verbatim");
        let initial = replace(&original, "Unknown", &[0xfe, 0, 0, 0, 1, 2, 3, 4]).unwrap();
        let changed = replace(&initial, "Mode", &[19, 0, 0, 0, 4, 0, 0, 0]).unwrap();
        let replaced = replace(&changed, "Mode", &[19, 0, 0, 0, 6, 0, 0, 0]).unwrap();
        assert_eq!(changed.len(), replaced.len());
        assert_eq!(&initial[..52], &replaced[..52]);
        assert!(replaced
            .windows(8)
            .any(|p| p == [0xfe, 0, 0, 0, 1, 2, 3, 4]));
        assert!(replaced.ends_with(b"\0\0\0\0other storage section, retained verbatim"));
        assert!(replace(&[0; 40], "Mode", &[]).is_none());
    }
    #[test]
    fn windows_reader_accepts_named_numbers_strings_and_streams_after_size_changes() {
        use super::super::{parse_storage, Value};
        let mut bytes = empty();
        for text in ["a much longer string", "short"] {
            bytes = replace(&bytes, "Text", &encode(&VARIANT::from(text)).unwrap()).unwrap();
        }
        bytes = replace(&bytes, "Mode", &encode(&VARIANT::from(3u32)).unwrap()).unwrap();
        let stream =
            unsafe { windows::Win32::UI::Shell::SHCreateMemStream(Some(&[1, 2, 3, 4, 5])) }
                .unwrap();
        let value = VARIANT::from(stream.cast::<windows::core::IUnknown>().unwrap());
        bytes = replace(&bytes, "Sort", &encode(&value).unwrap()).unwrap();
        let properties = parse_storage(&bytes).unwrap();
        assert!(properties
            .iter()
            .any(|(n, v)| n == "Text" && matches!(v,Value::Text(t) if t=="short")));
        assert!(properties
            .iter()
            .any(|(n, v)| n == "Mode" && matches!(v, Value::Number(3))));
        assert!(properties
            .iter()
            .any(|(n, v)| n == "Sort" && matches!(v,Value::Bytes(b) if b==&[1,2,3,4,5])));
        // Validate the wire format against Windows' deserializer, independently of
        // our legacy reader. Strings/odd-sized streams need DWORD payload padding.
        unsafe {
            use windows::Win32::UI::Shell::PropertiesSystem::{
                INamedPropertyStore, IPersistSerializedPropStorage, PSCreateMemoryPropertyStore,
                PCUSERIALIZEDPROPSTORAGE,
            };
            let mut raw = std::ptr::null_mut();
            PSCreateMemoryPropertyStore(&INamedPropertyStore::IID, &mut raw).unwrap();
            let store = INamedPropertyStore::from_raw(raw);
            let persist: IPersistSerializedPropStorage = store.cast().unwrap();
            persist
                .SetPropertyStorage(
                    PCUSERIALIZEDPROPSTORAGE(bytes[52..].as_ptr() as isize),
                    (bytes.len() - 52) as u32,
                )
                .unwrap();
            assert_eq!(
                store
                    .GetNamedValue(windows::core::w!("Text"))
                    .unwrap()
                    .to_string(),
                "short"
            );
            assert_eq!(
                store
                    .GetNamedValue(windows::core::w!("Mode"))
                    .unwrap()
                    .to_string(),
                "3"
            );
            assert!(store.GetNamedValue(windows::core::w!("Sort")).is_ok());
        }
    }
}
