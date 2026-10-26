//! Network namespace classification and read-only effective firewall group state.
//! No rule, profile, service, discovery or sharing setters are exposed here.
use windows::{
    core::{BSTR, GUID, PCWSTR},
    Win32::System::{
        Com::{
            CoCreateInstance, IDispatch, CLSCTX_INPROC_SERVER, DISPATCH_PROPERTYGET, DISPPARAMS,
        },
        Variant::{VariantToBoolean, VariantToInt32, VARIANT},
    },
};

/// A UNC computer is a shell folder, not a filesystem directory whose missing
/// attributes mean it should be replaced by This PC. Shares remain filesystem paths.
pub fn is_computer(path: &str) -> bool {
    let Some(tail) = path.strip_prefix(r"\\") else {
        return false;
    };
    let tail = tail.strip_suffix('\\').unwrap_or(tail);
    !tail.is_empty() && tail != "?" && tail != "." && !tail.contains(['\\', '/'])
}
#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub profiles: i32,
    pub discovery: bool,
    pub sharing: bool,
}
impl Status {
    pub fn resource(self) -> Option<u32> {
        notice(self.discovery, self.sharing)
    }
}
fn notice(discovery: bool, sharing: bool) -> Option<u32> {
    match (discovery, sharing) {
        (true, true) => None,
        (false, false) => Some(1036),
        (false, true) => Some(1047),
        (true, false) => Some(1048),
    }
}

/// Automation is intentionally PROPERTYGET only; it cannot call EnableRuleGroup,
/// restore defaults or write a firewall/profile property.
unsafe fn get(
    dispatch: &IDispatch,
    name: &str,
    mut args: Vec<VARIANT>,
) -> windows::core::Result<VARIANT> {
    let name = super::wide(name);
    let mut id = 0;
    dispatch.GetIDsOfNames(
        &GUID::zeroed(),
        [PCWSTR(name.as_ptr())].as_ptr(),
        1,
        0,
        &mut id,
    )?;
    let parameters = DISPPARAMS {
        rgvarg: if args.is_empty() {
            std::ptr::null_mut()
        } else {
            args.as_mut_ptr()
        },
        cArgs: args.len() as u32,
        ..Default::default()
    };
    let mut result = VARIANT::default();
    dispatch.Invoke(
        id,
        &GUID::zeroed(),
        0,
        DISPATCH_PROPERTYGET,
        &parameters,
        Some(&mut result),
        None,
        None,
    )?;
    Ok(result)
}
pub fn status() -> windows::core::Result<Status> {
    super::com_init();
    unsafe {
        let policy: IDispatch = CoCreateInstance(
            &GUID::from_u128(0xe2b3c97f_6ae1_41ac_817a_f6f92166d7dd),
            None,
            CLSCTX_INPROC_SERVER,
        )?;
        let profiles = VariantToInt32(&get(&policy, "CurrentProfileTypes", vec![])?)?;
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let group = |resource| -> windows::core::Result<bool> {
            let name = BSTR::from(format!(r"@{root}\System32\FirewallAPI.dll,-{resource}"));
            Ok(VariantToBoolean(&get(
                &policy,
                "IsRuleGroupCurrentlyEnabled",
                vec![VARIANT::from(name)],
            )?)?
            .as_bool())
        };
        Ok(Status {
            profiles,
            discovery: group(32752)?,
            sharing: group(28502)?,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unc_computers_are_distinct_from_shares_and_device_paths() {
        for path in [r"\\tsclient", r"\\server\", r"\\servidor com espaços"] {
            assert!(is_computer(path), "{path}");
        }
        for path in [
            r"C:\",
            r"\\server\share",
            r"\\server\share\folder",
            r"\\?\C:\",
            r"\\.\pipe\name",
            r"\\?",
            r"\\",
            r"\server",
        ] {
            assert!(!is_computer(path), "{path}");
        }
    }
    #[test]
    fn native_notice_variants_follow_effective_groups_not_individual_rules() {
        assert_eq!(notice(true, true), None);
        assert_eq!(notice(false, false), Some(1036));
        assert_eq!(notice(false, true), Some(1047));
        assert_eq!(notice(true, false), Some(1048));
    }
}
