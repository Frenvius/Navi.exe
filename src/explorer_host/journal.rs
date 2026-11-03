use serde::{Deserialize, Serialize};
use std::io;

pub(crate) const HOST: &str = r"Software\Classes\CLSID\{75DFF2B7-6936-4C06-A8BB-676A7B00B24B}";
pub(crate) const NATIVE: &str = r"Software\Classes\CLSID\{D7DFB84A-C031-49BC-AD7C-5064810A2E74}";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Value {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum View {
    Registry64,
    Registry32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Location {
    pub view: View,
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub location: Location,
    pub before: Option<Value>,
    pub after: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MachineJournal {
    pub version: u32,
    pub original_appid: [Value; 2],
    pub records: Vec<Record>,
}

pub(crate) trait Registry {
    fn read(&self, location: &Location) -> io::Result<Option<Value>>;
    fn write(&mut self, location: &Location, value: Option<&Value>) -> io::Result<()>;
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Explorer host journal")
}

fn appid(view: View) -> Location {
    Location {
        view,
        key: HOST.into(),
        name: "AppID".into(),
    }
}

impl MachineJournal {
    fn activation_order(&self) -> impl Iterator<Item = &Record> {
        self.records
            .iter()
            .filter(|record| !record.location.key.starts_with(HOST))
            .chain(
                self.records
                    .iter()
                    .filter(|record| record.location.key.starts_with(HOST)),
            )
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1 || self.records.len() != 6 {
            return Err(invalid());
        }
        let mut expected = Vec::new();
        for view in [View::Registry64, View::Registry32] {
            expected.extend([
                Location {
                    view,
                    key: format!(r"{HOST}\LocalServer32"),
                    name: String::new(),
                },
                Location {
                    view,
                    key: format!(r"{NATIVE}\LocalServer32"),
                    name: String::new(),
                },
                Location {
                    view,
                    key: NATIVE.into(),
                    name: "AppID".into(),
                },
            ]);
        }
        for (record, location) in self.records.iter().zip(expected) {
            if record.location != location
                || !matches!(record.after.kind, 1 | 2)
                || record.after.bytes.len() > 131072
                || record
                    .before
                    .as_ref()
                    .is_some_and(|v| v.kind > 11 || v.bytes.len() > 1_048_576)
            {
                return Err(invalid());
            }
        }
        for (index, original) in self.original_appid.iter().enumerate() {
            if original.kind != 1
                || original.bytes.len() > 256
                || self.records[index * 3 + 2].after != *original
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn unchanged_appid(&self, registry: &impl Registry) -> io::Result<()> {
        for (index, view) in [View::Registry64, View::Registry32].into_iter().enumerate() {
            if registry.read(&appid(view))? != Some(self.original_appid[index].clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Explorer AppID changed",
                ));
            }
        }
        Ok(())
    }

    pub fn apply(&self, registry: &mut impl Registry) -> io::Result<()> {
        self.validate()?;
        self.unchanged_appid(registry)?;
        for record in &self.records {
            if registry.read(&record.location)? != record.before {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Explorer registration changed",
                ));
            }
        }
        for record in self.activation_order() {
            registry.write(&record.location, Some(&record.after))?;
        }
        Ok(())
    }

    pub fn restore(&self, registry: &mut impl Registry) -> io::Result<()> {
        self.validate()?;
        self.unchanged_appid(registry)?;
        for record in &self.records {
            let actual = registry.read(&record.location)?;
            if actual != record.before && actual.as_ref() != Some(&record.after) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Foreign Explorer registration",
                ));
            }
        }
        for record in self
            .records
            .iter()
            .filter(|record| record.location.key.starts_with(HOST))
            .chain(
                self.records
                    .iter()
                    .rev()
                    .filter(|record| !record.location.key.starts_with(HOST)),
            )
        {
            registry.write(&record.location, record.before.as_ref())?;
        }
        Ok(())
    }

    pub fn repair(&self, registry: &mut impl Registry) -> io::Result<()> {
        self.validate()?;
        self.unchanged_appid(registry)?;
        for record in &self.records {
            let current = registry.read(&record.location)?;
            if current != record.before && current.as_ref() != Some(&record.after) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Foreign Explorer registration",
                ));
            }
        }
        for record in self.activation_order() {
            if registry.read(&record.location)?.as_ref() != Some(&record.after) {
                registry.write(&record.location, Some(&record.after))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Memory {
        values: BTreeMap<Location, Value>,
        fail_after: Option<usize>,
    }
    impl Registry for Memory {
        fn read(&self, location: &Location) -> io::Result<Option<Value>> {
            Ok(self.values.get(location).cloned())
        }
        fn write(&mut self, location: &Location, value: Option<&Value>) -> io::Result<()> {
            if let Some(left) = self.fail_after.as_mut() {
                if *left == 0 {
                    self.fail_after = None;
                    return Err(io::Error::other("Injected write failure"));
                }
                *left -= 1;
            }
            if let Some(value) = value {
                self.values.insert(location.clone(), value.clone());
            } else {
                self.values.remove(location);
            }
            Ok(())
        }
    }
    fn value(text: &str) -> Value {
        Value {
            kind: 1,
            bytes: text
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    }
    fn fixture() -> (MachineJournal, Memory) {
        let mut registry = Memory::default();
        let original = value("{ORIGINAL-APPID}");
        let mut records = Vec::new();
        for view in [View::Registry64, View::Registry32] {
            registry.write(&appid(view), Some(&original)).unwrap();
            for (key, name, after) in [
                (
                    format!(r"{HOST}\LocalServer32"),
                    "",
                    value(r#""C:\Program Files\Navi\navi-explorer-host.exe" --serve"#),
                ),
                (
                    format!(r"{NATIVE}\LocalServer32"),
                    "",
                    value("explorer.exe /factory,{D7DFB84A-C031-49BC-AD7C-5064810A2E74}"),
                ),
                (NATIVE.into(), "AppID", original.clone()),
            ] {
                let location = Location {
                    view,
                    key,
                    name: name.into(),
                };
                let before = if key_is_host(&location) {
                    Some(Value {
                        kind: 2,
                        bytes: vec![255, 0, 42, 0],
                    })
                } else {
                    None
                };
                registry.write(&location, before.as_ref()).unwrap();
                records.push(Record {
                    location,
                    before,
                    after,
                });
            }
        }
        (
            MachineJournal {
                version: 1,
                original_appid: [original.clone(), original],
                records,
            },
            registry,
        )
    }
    fn key_is_host(location: &Location) -> bool {
        location.key.starts_with(HOST)
    }

    #[test]
    fn both_views_raw_bytes_missing_values_and_appid_roundtrip() {
        let (journal, mut registry) = fixture();
        let before = registry.values.clone();
        let journal: MachineJournal =
            serde_json::from_slice(&serde_json::to_vec(&journal).unwrap()).unwrap();
        journal.apply(&mut registry).unwrap();
        journal.restore(&mut registry).unwrap();
        journal.restore(&mut registry).unwrap();
        assert_eq!(before, registry.values);
    }
    #[test]
    fn install_and_repair_prepare_both_native_aliases_before_arming_either_host() {
        struct Guard<'a> {
            inner: Memory,
            journal: &'a MachineJournal,
        }
        impl Registry for Guard<'_> {
            fn read(&self, location: &Location) -> io::Result<Option<Value>> {
                self.inner.read(location)
            }
            fn write(&mut self, location: &Location, value: Option<&Value>) -> io::Result<()> {
                if location.key.starts_with(HOST) {
                    for alias in self
                        .journal
                        .records
                        .iter()
                        .filter(|record| record.location.key.starts_with(NATIVE))
                    {
                        assert_eq!(
                            self.inner.read(&alias.location).unwrap().as_ref(),
                            Some(&alias.after)
                        );
                    }
                }
                self.inner.write(location, value)
            }
        }
        let (journal, registry) = fixture();
        let mut guarded = Guard {
            inner: registry,
            journal: &journal,
        };
        journal.apply(&mut guarded).unwrap();
        let (journal, registry) = fixture();
        let mut guarded = Guard {
            inner: registry,
            journal: &journal,
        };
        journal.repair(&mut guarded).unwrap();
    }
    #[test]
    fn every_partial_install_can_restore() {
        for count in 0..6 {
            let (journal, mut registry) = fixture();
            let before = registry.values.clone();
            registry.fail_after = Some(count);
            assert!(journal.apply(&mut registry).is_err());
            journal.restore(&mut registry).unwrap();
            assert_eq!(before, registry.values);
        }
    }
    #[test]
    fn foreign_claim_or_appid_change_causes_zero_rollback_writes() {
        for appid_changed in [false, true] {
            let (journal, mut registry) = fixture();
            journal.apply(&mut registry).unwrap();
            let location = if appid_changed {
                appid(View::Registry32)
            } else {
                journal.records[5].location.clone()
            };
            registry.write(&location, Some(&value("foreign"))).unwrap();
            let before = registry.values.clone();
            assert!(journal.restore(&mut registry).is_err());
            assert_eq!(before, registry.values);
        }
    }
    #[test]
    fn scope_order_version_and_alias_appid_are_validated_before_write() {
        for change in 0..5 {
            let (mut journal, mut registry) = fixture();
            let before = registry.values.clone();
            match change {
                0 => journal.records[0].location.key = r"Software\Microsoft".into(),
                1 => journal.records.swap(0, 3),
                2 => journal.version = 2,
                3 => journal.records[2].after = value("different-appid"),
                _ => journal.records.pop().map(|_| ()).unwrap(),
            }
            assert!(journal.apply(&mut registry).is_err());
            assert_eq!(before, registry.values);
        }
    }
    #[test]
    fn repair_accepts_original_or_owned_values_but_refuses_foreign_claims() {
        let (journal, mut registry) = fixture();
        journal.apply(&mut registry).unwrap();
        registry
            .write(
                &journal.records[0].location,
                journal.records[0].before.as_ref(),
            )
            .unwrap();
        journal.repair(&mut registry).unwrap();
        journal.repair(&mut registry).unwrap();
        assert_eq!(
            registry.read(&journal.records[0].location).unwrap(),
            Some(journal.records[0].after.clone())
        );
        registry
            .write(
                &journal.records[3].location,
                Some(&value("foreign-manager")),
            )
            .unwrap();
        let before = registry.values.clone();
        assert!(journal.repair(&mut registry).is_err());
        assert_eq!(before, registry.values);
    }
}
