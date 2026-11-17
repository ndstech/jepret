//! Minimal read-only registry helpers (Windows) for "Open with" discovery.
//! Everything here is best-effort: any failure yields `None` / empty.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumValueW, RegGetValueW, RegOpenKeyExW, HKEY, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read the (default) string value of `root\subkey` (REG_EXPAND_SZ is expanded).
pub fn default_string(root: HKEY, subkey: &str) -> Option<String> {
    let sub = wide(subkey);
    let mut size = 0u32;
    // SAFETY: `sub` is NUL-terminated and outlives the call; size query only.
    let rc = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS || size < 2 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `buf` holds at least `size` bytes, which is passed as the capacity.
    let rc = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

/// Enumerate values of `root\subkey` as `(name, string data)`; non-string data is "".
pub fn enum_values(root: HKEY, subkey: &str) -> Vec<(String, String)> {
    let sub = wide(subkey);
    let mut key = HKEY::default();
    // SAFETY: valid NUL-terminated path and out pointer.
    let rc = unsafe { RegOpenKeyExW(root, PCWSTR(sub.as_ptr()), 0, KEY_READ, &mut key) };
    if rc != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for index in 0u32..4096 {
        let mut name = vec![0u16; 512];
        let mut name_len = name.len() as u32;
        let mut data = vec![0u8; 2048];
        let mut data_len = data.len() as u32;
        let mut ty = 0u32;
        // SAFETY: buffers are sized per the lengths passed alongside them.
        let rc = unsafe {
            RegEnumValueW(
                key,
                index,
                PWSTR(name.as_mut_ptr()),
                &mut name_len,
                None,
                Some(&mut ty),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if rc == ERROR_MORE_DATA {
            continue; // oversized value: skip it
        }
        if rc != ERROR_SUCCESS {
            break; // ERROR_NO_MORE_ITEMS or real failure
        }
        let n = String::from_utf16_lossy(&name[..name_len as usize]);
        let s = if ty == REG_SZ.0 || ty == REG_EXPAND_SZ.0 {
            let u16s: Vec<u16> = data[..data_len as usize]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let end = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
            String::from_utf16_lossy(&u16s[..end])
        } else {
            String::new()
        };
        out.push((n, s));
    }
    // SAFETY: `key` was opened above and is closed exactly once.
    unsafe {
        let _ = RegCloseKey(key);
    }
    out
}
