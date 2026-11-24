use std::fs;
use std::path::PathBuf;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

const CONFIG_FILE_NAME: &str = "taskbar_config.json";
const REG_APP_NAME: &str = "TENET_TaskbarControl\0";

// --- Settings Persistence ---

pub fn get_config_path() -> PathBuf {
    // %APPDATA%\TENET_TaskbarControl\taskbar_config.json (always writable).
    // Falls back to the exe folder if APPDATA is unavailable.
    match std::env::var_os("APPDATA") {
        Some(appdata) => {
            let mut dir = PathBuf::from(appdata);
            dir.push("TENET_TaskbarControl");
            let _ = fs::create_dir_all(&dir);
            dir.push(CONFIG_FILE_NAME);
            dir
        }
        None => {
            let mut path = std::env::current_exe().unwrap_or_default();
            path.set_file_name(CONFIG_FILE_NAME);
            path
        }
    }
}

pub fn save_config_to_disk(json_str: &str) -> Result<(), std::io::Error> {
    // Atomic write: temp file first, then replace, so a crash can't corrupt the config.
    let path = get_config_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json_str)?;
    fs::rename(&tmp, &path)
}

pub fn load_config_from_disk() -> Option<String> {
    let path = get_config_path();
    if path.exists() {
        fs::read_to_string(path).ok()
    } else {
        None
    }
}

// --- Windows Startup Registry Manager ---

pub fn is_startup_enabled() -> bool {
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
        .encode_utf16()
        .collect();
    let value_name: Vec<u16> = REG_APP_NAME.encode_utf16().collect();

    unsafe {
        let mut hkey = 0;
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        );

        if status != 0 {
            return false;
        }

        let mut data_type = 0;
        let mut data_len = 0;
        let query_res = RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut data_type,
            std::ptr::null_mut(),
            &mut data_len,
        );

        RegCloseKey(hkey);
        query_res == 0
    }
}

pub fn set_startup_enabled(enabled: bool) -> bool {
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
        .encode_utf16()
        .collect();
    let value_name: Vec<u16> = REG_APP_NAME.encode_utf16().collect();

    unsafe {
        let mut hkey = 0;
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_WRITE,
            &mut hkey,
        );

        if status != 0 {
            return false;
        }

        let result = if enabled {
            if let Ok(exe_path) = std::env::current_exe() {
                let path_str = format!("\"{}\"\0", exe_path.to_string_lossy());
                let path_wide: Vec<u16> = path_str.encode_utf16().collect();
                RegSetValueExW(
                    hkey,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    path_wide.as_ptr() as *const u8,
                    (path_wide.len() * 2) as u32,
                ) == 0
            } else {
                false
            }
        } else {
            RegDeleteValueW(hkey, value_name.as_ptr()) == 0
        };

        RegCloseKey(hkey);
        result
    }
}