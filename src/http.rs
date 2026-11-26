use anyhow::{Context, Result, bail};
use std::{
    ffi::c_void,
    io::{self, Read},
    ptr::{null, null_mut},
};
use url::Url;
use windows_sys::Win32::Networking::WinHttp::*;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

struct Handle(usize);

impl Handle {
    fn new(value: *mut c_void) -> Result<Self> {
        if value.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(value as usize))
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0 as *mut c_void
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.as_ptr());
        }
    }
}

pub struct Response {
    request: Handle,
    _connect: Handle,
    _session: Handle,
}

impl Response {
    pub fn open(url: &Url, headers: &str, timeout_ms: i32) -> Result<Self> {
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("Invalid stream URL");
        }
        let hostname = url.host_str().context("URL has no hostname")?;
        let host = wide(hostname);
        let port = url.port_or_known_default().context("URL has no port")?;
        let path = wide(&match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        });
        let proxy = match hostname {
            "127.0.0.1" | "localhost" | "[::1]" => WINHTTP_ACCESS_TYPE_NO_PROXY,
            _ => WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        };
        let session = Handle::new(unsafe {
            WinHttpOpen(
                wide(concat!("DesktopRadio/", env!("CARGO_PKG_VERSION"))).as_ptr(),
                proxy,
                null(),
                null(),
                0,
            )
        })?;
        check(unsafe {
            WinHttpSetTimeouts(
                session.as_ptr(),
                timeout_ms,
                timeout_ms,
                timeout_ms,
                timeout_ms,
            )
        })?;
        let connect =
            Handle::new(unsafe { WinHttpConnect(session.as_ptr(), host.as_ptr(), port, 0) })?;
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(
                connect.as_ptr(),
                null(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                if url.scheme() == "https" {
                    WINHTTP_FLAG_SECURE
                } else {
                    0
                },
            )
        })?;
        let headers = wide(headers);
        check(unsafe {
            WinHttpSendRequest(
                request.as_ptr(),
                headers.as_ptr(),
                (headers.len() - 1) as u32,
                null(),
                0,
                0,
                0,
            )
        })?;
        check(unsafe { WinHttpReceiveResponse(request.as_ptr(), null_mut()) })?;
        let response = Self {
            request,
            _connect: connect,
            _session: session,
        };
        let mut status = 0_u32;
        let mut bytes = 4;
        check(unsafe {
            WinHttpQueryHeaders(
                response.request.as_ptr(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut bytes,
                null_mut(),
            )
        })?;
        if !(200..300).contains(&status) {
            bail!("HTTP {status}");
        }
        Ok(response)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let mut value = [0_u16; 1024];
        let mut bytes = size_of_val(&value) as u32;
        let result = unsafe {
            WinHttpQueryHeaders(
                self.request.as_ptr(),
                WINHTTP_QUERY_CUSTOM,
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                &mut bytes,
                null_mut(),
            )
        };
        (result != 0).then(|| {
            String::from_utf16_lossy(&value[..bytes as usize / 2])
                .trim_end_matches('\0')
                .to_owned()
        })
    }

    pub fn effective_url(&self, fallback: &Url) -> Url {
        let mut value = [0_u16; 4096];
        let mut bytes = size_of_val(&value) as u32;
        let result = unsafe {
            WinHttpQueryOption(
                self.request.as_ptr(),
                WINHTTP_OPTION_URL,
                value.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if result != 0 {
            if let Ok(url) = Url::parse(
                String::from_utf16_lossy(&value[..bytes as usize / 2]).trim_end_matches('\0'),
            ) {
                return url;
            }
        }
        fallback.clone()
    }

    pub fn collect(self, limit: u64) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        self.take(limit + 1).read_to_end(&mut body)?;
        if body.len() as u64 > limit {
            bail!("Response exceeded size limit");
        }
        Ok(body)
    }
}

impl Read for Response {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        let mut count = 0;
        let length = target.len().min(16 * 1024) as u32;
        check(unsafe {
            WinHttpReadData(
                self.request.as_ptr(),
                target.as_mut_ptr().cast(),
                length,
                &mut count,
            )
        })?;
        Ok(count as usize)
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
