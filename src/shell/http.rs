use std::io;
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::Networking::WinHttp::*,
};

struct Handle(*mut core::ffi::c_void);

impl Handle {
    fn new(raw: *mut core::ffi::c_void) -> io::Result<Self> {
        if raw.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn split(url: &str) -> io::Result<(&str, &str)> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| io::Error::other(format!("not an https URL: {url}")))?;
    Ok(match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, "/"),
    })
}

pub fn get(url: &str, accept: &str, limit: usize) -> io::Result<Vec<u8>> {
    let (host, path) = split(url)?;
    let agent = HSTRING::from(format!("Navi/{}", env!("CARGO_PKG_VERSION")));
    unsafe {
        let session = Handle::new(WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000)?;
        let connection = Handle::new(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = Handle::new(WinHttpOpenRequest(
            connection.0,
            &HSTRING::from("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut size,
            std::ptr::null_mut(),
        )?;
        if status != 200 {
            return Err(io::Error::other(format!("HTTP {status} for {url}")));
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr() as *mut _,
                chunk.len() as u32,
                &mut read,
            )?;
            if read == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > limit {
                return Err(io::Error::other(format!("response over {limit} bytes")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_https_urls_and_rejects_others() {
        assert_eq!(
            split("https://api.github.com/repos/a/b/releases/latest").unwrap(),
            ("api.github.com", "/repos/a/b/releases/latest")
        );
        assert_eq!(split("https://example.com").unwrap(), ("example.com", "/"));
        assert!(split("http://example.com/x").is_err());
    }
}
