#![windows_subsystem = "windows"]
use std::time::Instant;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

// --- Wry & Tao Windowing / Tray Imports ---
use wry::application::dpi::{LogicalSize, PhysicalPosition};
use wry::application::event::{Event, WindowEvent};
use wry::application::event_loop::{ControlFlow, EventLoop};
use wry::application::system_tray::SystemTrayBuilder;
use wry::application::window::WindowBuilder;
use wry::webview::WebViewBuilder;

// --- Win32 Low-Level Imports ---
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT};
use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Accessibility::SetWinEventHook;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetCursorPos,
    GetMessageW, GetWindowLongW, IsIconic, IsWindowVisible, IsZoomed, KillTimer, LoadIconW,
    SendMessageW, SetTimer, SetWindowPos, TranslateMessage, GWL_EXSTYLE, ICON_BIG, ICON_SMALL,
    MSG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_SETICON,
    WS_EX_TOOLWINDOW,
};

mod features;

// --- Undocumented Win32 Composition Structs ---

#[repr(C)]
#[allow(non_snake_case)]
struct ACCENT_POLICY {
    AccentState: i32,
    AccentFlags: i32,
    GradientColor: u32,
    AnimationId: i32,
}

#[repr(C)]
#[allow(non_snake_case)]
struct WINDOWCOMPOSITIONATTRIBUTEDATA {
    Attribute: i32,
    Data: *mut std::ffi::c_void,
    SizeOfData: usize,
}

type SetWindowCompositionAttributeFn = unsafe extern "system" fn(
    hwnd: HWND,
    data: *mut WINDOWCOMPOSITIONATTRIBUTEDATA,
) -> i32;

const ACCENT_DISABLED: i32 = 0;
const ACCENT_ENABLE_GRADIENT: i32 = 1;
const ACCENT_ENABLE_TRANSPARENTGRADIENT: i32 = 2;
const ACCENT_ENABLE_BLURBEHIND: i32 = 3;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: i32 = 4;

unsafe fn get_set_window_composition_attribute() -> Option<SetWindowCompositionAttributeFn> {
    let module_name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
    let handle = GetModuleHandleW(module_name.as_ptr());
    if handle == 0 {
        return None;
    }

    let proc_name = b"SetWindowCompositionAttribute\0";
    let proc = GetProcAddress(handle, proc_name.as_ptr());
    if proc.is_none() {
        return None;
    }

    Some(std::mem::transmute(proc))
}

// --- Win32 Immersive Dark Titlebar Helper ---

unsafe fn apply_dark_titlebar(hwnd: HWND) {
    let module_name: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
    let handle = LoadLibraryW(module_name.as_ptr());
    if handle != 0 {
        let proc_name = b"DwmSetWindowAttribute\0";
        if let Some(proc) = GetProcAddress(handle, proc_name.as_ptr()) {
            type DwmSetWindowAttributeFn =
                unsafe extern "system" fn(HWND, u32, *const std::ffi::c_void, u32) -> i32;
            let func: DwmSetWindowAttributeFn = std::mem::transmute(proc);
            let dark_mode: i32 = 1;
            let _ = func(
                hwnd,
                20,
                &dark_mode as *const _ as *const _,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

// --- App State ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateConfig {
    pub state: String,
    pub hex_color: String,
    pub opacity: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub desktop: StateConfig,
    pub maximized: StateConfig,
    pub maximized_enabled: bool,
    pub startup_enabled: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            desktop: StateConfig {
                state: "clear".to_string(),
                hex_color: "#000000".to_string(),
                opacity: 0,
            },
            maximized: StateConfig {
                state: "opaque".to_string(),
                hex_color: "#10121a".to_string(),
                opacity: 100,
            },
            maximized_enabled: false,
            startup_enabled: false,
        }
    }
}

// --- IPC Deserialization ---

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum IpcMessage {
    #[serde(rename = "UPDATE_CONFIG")]
    UpdateConfig {
        target: String,
        enabled: bool,
        state: String,
        hex_color: String,
        opacity: u32,
    },
    #[serde(rename = "SET_STARTUP")]
    SetStartup { enabled: bool },
}

// --- Custom Themed Tray Menu HTML ---

const TRAY_MENU_HTML: &str = r#"
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@500;700&display=swap" rel="stylesheet">
  <style>
    :root {
      --bg: #050505;
      --border: #1a1a1a;
      --accent: #00ff88;
      --text: #f3f4f6;
      --subtext: #8a8e9b;
      --danger: #ff4444;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; font-family: 'JetBrains Mono', monospace; user-select: none; }
    html, body {
      background: transparent;
      width: 100%;
      height: 100%;
      overflow: hidden;
    }
    .menu-container {
      background: rgba(5, 5, 5, 0.96);
      backdrop-filter: blur(16px);
      border: 1px solid var(--accent);
      box-shadow: 0 0 20px rgba(0, 255, 136, 0.2);
      border-radius: 8px;
      padding: 6px;
      display: flex;
      flex-direction: column;
      gap: 4px;
      height: 100%;
      animation: popupFade 0.15s cubic-bezier(0.16, 1, 0.3, 1);
    }
    @keyframes popupFade {
      from { opacity: 0; transform: translateY(4px) scale(0.96); }
      to { opacity: 1; transform: translateY(0) scale(1); }
    }
    .menu-header {
      font-size: 0.58rem;
      color: var(--subtext);
      padding: 4px 8px;
      letter-spacing: 1px;
      text-transform: uppercase;
      border-bottom: 1px solid var(--border);
      margin-bottom: 2px;
    }
    .menu-item {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 7px 10px;
      font-size: 0.72rem;
      color: var(--text);
      border-radius: 5px;
      cursor: pointer;
      transition: all 0.18s ease;
    }
    .menu-item:hover {
      background: rgba(0, 255, 136, 0.12);
      color: var(--accent);
      padding-left: 14px;
    }
    .menu-item.exit:hover {
      background: rgba(255, 68, 68, 0.12);
      color: var(--danger);
      padding-left: 14px;
    }
    .dot {
      width: 6px;
      height: 6px;
      border-radius: 50%;
      background: var(--accent);
      box-shadow: 0 0 6px var(--accent);
      flex-shrink: 0;
    }
    .dot.red {
      background: var(--danger);
      box-shadow: 0 0 6px var(--danger);
    }
  </style>
</head>
<body>
  <div class="menu-container">
    <div class="menu-header">TENET BEND ⧉</div>
    <div class="menu-item" onclick="sendAction('TOGGLE')">
      <span class="dot"></span>
      <span>Show / Hide</span>
    </div>
    <div class="menu-item exit" onclick="sendAction('QUIT')">
      <span class="dot red"></span>
      <span>Exit Panel</span>
    </div>
  </div>
  <script>
    function sendAction(act) {
      if (window.ipc) { window.ipc.postMessage(act); }
    }
  </script>
</body>
</html>
"#;

// --- Accent State & Color Logic ---

fn calculate_accent_and_color(state: &str, hex_color: &str, opacity_percent: u32) -> (i32, u32) {
    let hex = hex_color.trim_start_matches('#');
    let mut r = 0u32;
    let mut g = 0u32;
    let mut b = 0u32;

    if hex.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(hex, 16) {
            r = (rgb >> 16) & 0xFF;
            g = (rgb >> 8) & 0xFF;
            b = rgb & 0xFF;
        }
    }

    match state {
        "clear" => {
            let a = ((opacity_percent as f32 / 100.0) * 255.0) as u32;
            let argb = (a << 24) | (b << 16) | (g << 8) | r;
            (ACCENT_ENABLE_TRANSPARENTGRADIENT, argb)
        }
        "opaque" => {
            // Opaque means solid: alpha is always 255 (the slider is ignored here).
            // Use Acrylic/Blur if you want a translucent tint.
            let a = 255u32;
            let argb = (a << 24) | (b << 16) | (g << 8) | r;
            (ACCENT_ENABLE_GRADIENT, argb)
        }
        "acrylic" => {
            let a = if opacity_percent == 0 {
                1
            } else {
                ((opacity_percent as f32 / 100.0) * 255.0) as u32
            };
            let argb = (a << 24) | (b << 16) | (g << 8) | r;
            (ACCENT_ENABLE_ACRYLICBLURBEHIND, argb)
        }
        "blur" => {
            let a = ((opacity_percent as f32 / 100.0) * 255.0) as u32;
            let argb = (a << 24) | (b << 16) | (g << 8) | r;
            (ACCENT_ENABLE_BLURBEHIND, argb)
        }
        "normal" | "default" => (ACCENT_DISABLED, 0x00000000),
        _ => (ACCENT_DISABLED, 0x00000000),
    }
}

// `redraw = true` forces a frame redraw (only needed when the user changes a setting).
// Background re-applies use `redraw = false` so there is no flicker and almost no cost.
fn apply_taskbar_state(accent_state: i32, color_argb: u32, redraw: bool) {
    let mut policy = ACCENT_POLICY {
        AccentState: accent_state,
        AccentFlags: 2,
        GradientColor: color_argb,
        AnimationId: 0,
    };

    let mut data = WINDOWCOMPOSITIONATTRIBUTEDATA {
        Attribute: 19,
        Data: &mut policy as *mut _ as *mut std::ffi::c_void,
        SizeOfData: std::mem::size_of::<ACCENT_POLICY>(),
    };

    let redraw_flags =
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;

    unsafe {
        let set_composition = match get_set_window_composition_attribute() {
            Some(func) => func,
            None => return,
        };

        // Primary Taskbar
        let class_main: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
        let hwnd_main = FindWindowW(class_main.as_ptr(), std::ptr::null());
        if hwnd_main != 0 {
            set_composition(hwnd_main, &mut data);
            if redraw {
                SetWindowPos(hwnd_main, 0, 0, 0, 0, 0, redraw_flags);
            }
        }

        // Secondary Taskbars
        let class_sec: Vec<u16> = "Shell_SecondaryTrayWnd\0".encode_utf16().collect();
        let mut hwnd_sec = FindWindowW(class_sec.as_ptr(), std::ptr::null());
        while hwnd_sec != 0 {
            set_composition(hwnd_sec, &mut data);
            if redraw {
                SetWindowPos(hwnd_sec, 0, 0, 0, 0, 0, redraw_flags);
            }
            hwnd_sec = FindWindowExW(0, hwnd_sec, class_sec.as_ptr(), std::ptr::null());
        }
    }
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn is_taskbar_class(hwnd: HWND) -> bool {
    let c = class_name(hwnd);
    c == "Shell_TrayWnd" || c == "Shell_SecondaryTrayWnd"
}

unsafe extern "system" fn enum_maximized_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    // Must be a visible, non-minimized, maximized window
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || IsZoomed(hwnd) == 0 {
        return 1;
    }

    // Skip tool windows
    if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }

    // Skip cloaked windows (windows living on other virtual desktops, suspended UWP, etc.)
    let mut cloaked: u32 = 0;
    if DwmGetWindowAttribute(hwnd, 14, &mut cloaked as *mut u32 as *mut _, 4) == 0 && cloaked != 0 {
        return 1;
    }

    // Skip the shell itself
    let class = class_name(hwnd);
    if matches!(
        class.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    ) {
        return 1;
    }

    *(lparam as *mut bool) = true;
    0 // stop enumerating
}

unsafe fn is_any_window_maximized() -> bool {
    let mut found = false;
    EnumWindows(Some(enum_maximized_proc), &mut found as *mut bool as LPARAM);
    found
}

fn resolve_target(config: &AppConfig) -> (i32, u32) {
    let active_state = unsafe {
        if config.maximized_enabled && is_any_window_maximized() {
            &config.maximized
        } else {
            &config.desktop
        }
    };

    calculate_accent_and_color(
        &active_state.state,
        &active_state.hex_color,
        active_state.opacity,
    )
}

// Used when the user changes a setting: applies and forces a redraw.
fn evaluate_and_apply(config: &AppConfig) {
    let (accent, argb) = resolve_target(config);
    apply_taskbar_state(accent, argb, true);
}

// Used by the background engine: applies silently, no redraw, no flicker.
fn evaluate_quiet(config: &AppConfig) {
    let (accent, argb) = resolve_target(config);
    apply_taskbar_state(accent, argb, false);
}

// --- Watchdog Thread ---

// Event-driven persistence: reacts to the things that make Explorer reset the taskbar
// (Start menu, virtual desktop switch, minimize/restore, driver reset, Explorer restart)
// instead of blindly hammering the taskbar every few hundred milliseconds.

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_SYSTEM_DESKTOPSWITCH: u32 = 0x0020;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const EVENT_OBJECT_CLOAKED: u32 = 0x8017;
const EVENT_OBJECT_UNCLOAKED: u32 = 0x8018;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WINEVENT_SKIPOWNPROCESS: u32 = 2;

// Explorer often overwrites our effect a moment AFTER the event, so re-apply in a short burst.
const BURST_DELAYS_MS: [u32; 3] = [60, 200, 600];
// Safety net for events Windows doesn't announce (Win+Ctrl+Shift+B, Explorer restart).
const HEARTBEAT_MS: u32 = 1000;

static CONFIG: OnceLock<Arc<Mutex<AppConfig>>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();
static BURST_TIMER_ID: AtomicUsize = AtomicUsize::new(0);
static BURST_STEP: AtomicUsize = AtomicUsize::new(0);
static LAST_MAXIMIZED: AtomicBool = AtomicBool::new(false);
static LAST_CHECK_MS: AtomicU32 = AtomicU32::new(0);

fn reapply_now() {
    if let Some(cfg) = CONFIG.get() {
        if let Ok(c) = cfg.lock() {
            evaluate_quiet(&c);
        }
    }
}

unsafe fn arm_burst_step(step: usize) {
    let id = SetTimer(
        0,
        BURST_TIMER_ID.load(Ordering::Relaxed),
        BURST_DELAYS_MS[step],
        Some(burst_timer_proc),
    );
    BURST_TIMER_ID.store(id, Ordering::Relaxed);
}

unsafe extern "system" fn burst_timer_proc(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    reapply_now();
    let next = BURST_STEP.fetch_add(1, Ordering::Relaxed) + 1;
    if next < BURST_DELAYS_MS.len() {
        arm_burst_step(next);
    } else {
        let id = BURST_TIMER_ID.swap(0, Ordering::Relaxed);
        KillTimer(0, id);
    }
}

unsafe fn schedule_burst() {
    BURST_STEP.store(0, Ordering::Relaxed);
    arm_burst_step(0);
}

unsafe extern "system" fn heartbeat_proc(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    reapply_now();
}

unsafe extern "system" fn win_event_proc(
    _hook: isize,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Only whole-window events matter (OBJID_WINDOW == 0); this also filters cursor spam.
    if id_object != 0 {
        return;
    }

    if event == EVENT_OBJECT_LOCATIONCHANGE {
        // Taskbar itself changed -> something touched it, restore our look.
        if hwnd != 0 && is_taskbar_class(hwnd) {
            reapply_now();
            schedule_burst();
            return;
        }

        // Some other window moved/resized: only matters if it flips the maximized state.
        let enabled = CONFIG
            .get()
            .and_then(|c| c.lock().ok().map(|g| g.maximized_enabled))
            .unwrap_or(false);
        if !enabled {
            return;
        }

        let now = START.get().map(|s| s.elapsed().as_millis() as u32).unwrap_or(0);
        if now.wrapping_sub(LAST_CHECK_MS.load(Ordering::Relaxed)) < 120 {
            return;
        }
        LAST_CHECK_MS.store(now, Ordering::Relaxed);

        let max_now = is_any_window_maximized();
        if LAST_MAXIMIZED.swap(max_now, Ordering::Relaxed) != max_now {
            reapply_now();
            schedule_burst();
        }
        return;
    }

    // Foreground change, minimize/restore, cloak/uncloak (virtual desktops, Start menu)
    reapply_now();
    schedule_burst();
}

fn start_event_engine(config: Arc<Mutex<AppConfig>>) {
    let _ = CONFIG.set(config);
    let _ = START.set(Instant::now());

    thread::spawn(|| unsafe {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let ranges: [(u32, u32); 5] = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_DESKTOPSWITCH),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];
        for (lo, hi) in ranges {
            SetWinEventHook(lo, hi, 0, Some(win_event_proc), 0, 0, flags);
        }

        SetTimer(0, 0, HEARTBEAT_MS, Some(heartbeat_proc));
        reapply_now();

        // WinEvent hooks and timers are delivered through this thread's message loop.
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

// --- Main Entry Point ---

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "--log-level=3");

    // Single-instance guard: a second copy would fight the first over the taskbar.
    let mutex_name: Vec<u16> = "Local\\TENET_TaskbarControl_Singleton\0".encode_utf16().collect();
    let _instance_guard = unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr());
        if handle != 0 && GetLastError() == 183 {
            // ERROR_ALREADY_EXISTS
            return Ok(());
        }
        handle
    };

    let config = Arc::new(Mutex::new(AppConfig::default()));

    // Synchronize startup state directly from Windows Registry
    if let Ok(mut cfg) = config.lock() {
        cfg.startup_enabled = features::is_startup_enabled();
    }

    if let Some(saved_json) = features::load_config_from_disk() {
        if let Ok(parsed_cfg) = serde_json::from_str::<AppConfig>(&saved_json) {
            if let Ok(mut cfg) = config.lock() {
                *cfg = parsed_cfg;
                cfg.startup_enabled = features::is_startup_enabled();
            }
        }
    }

    start_event_engine(Arc::clone(&config));

    let event_loop = EventLoop::new();

    // Embed icon bytes for the system tray
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.ico");
    let tray_icon = match image::load_from_memory(ICON_BYTES) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            wry::application::window::Icon::from_rgba(rgba.into_raw(), width, height)
                .unwrap_or_else(|_| wry::application::window::Icon::from_rgba(vec![0; 16 * 16 * 4], 16, 16).unwrap())
        }
        Err(_) => wry::application::window::Icon::from_rgba(vec![0; 16 * 16 * 4], 16, 16).unwrap(),
    };

    // Build system tray with NO native menu (None triggers custom click events)
    let _system_tray = SystemTrayBuilder::new(tray_icon, None).build(&event_loop)?;

    // --- Main Panel Window ---
    let main_window = WindowBuilder::new()
        .with_title("Taskbar Control Panel")
        .with_inner_size(LogicalSize::new(660.0, 480.0))
        .with_visible(false)
        .build(&event_loop)?;

    #[cfg(target_os = "windows")]
    unsafe {
        use wry::application::platform::windows::WindowExtWindows;
        let hwnd = main_window.hwnd() as HWND;

        apply_dark_titlebar(hwnd);

        // Send WM_SETICON using IDI_ICON1 (ID 1) compiled directly into .exe by build.rs
        let hinstance = GetModuleHandleW(std::ptr::null());
        let icon_handle = LoadIconW(hinstance, 1 as *const u16);

        if icon_handle != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon_handle as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon_handle as isize);
        }
    }

    let main_html = include_str!("../ui/index.html");
    
    // Create an Arc clone for the main webview
    let config_ipc_main = Arc::clone(&config);

    // 2. Pass main_window by value
    let initial_script = {
        let json = config
            .lock()
            .ok()
            .and_then(|c| serde_json::to_string(&*c).ok())
            .unwrap_or_else(|| "null".to_string());
        format!("window.__INITIAL_CONFIG__ = {};", json)
    };

    let main_webview = WebViewBuilder::new(main_window)?
        .with_initialization_script(&initial_script)
        .with_html(main_html)?
        .with_ipc_handler(move |_window, req| {
            if let Ok(msg) = serde_json::from_str::<IpcMessage>(&req) {
                match msg {
                    IpcMessage::UpdateConfig {
                        target,
                        enabled,
                        state,
                        hex_color,
                        opacity,
                    } => {
                        if let Ok(mut cfg) = config_ipc_main.lock() {
                            if target == "maximized" {
                                cfg.maximized_enabled = enabled;
                                cfg.maximized = StateConfig {
                                    state,
                                    hex_color,
                                    opacity,
                                };
                            } else {
                                cfg.desktop = StateConfig {
                                    state,
                                    hex_color,
                                    opacity,
                                };
                            }

                            evaluate_and_apply(&cfg);

                            if let Ok(serialized) = serde_json::to_string(&*cfg) {
                                let _ = features::save_config_to_disk(&serialized);
                            }
                        }
                    }
                    IpcMessage::SetStartup { enabled } => {
                        let _success = features::set_startup_enabled(enabled);
                        if let Ok(mut cfg) = config_ipc_main.lock() {
                            cfg.startup_enabled = features::is_startup_enabled();
                            if let Ok(serialized) = serde_json::to_string(&*cfg) {
                                let _ = features::save_config_to_disk(&serialized);
                            }
                        }
                    }
                }
            }
        })
        .build()?;

    // Apply saved state on launch (the UI hydrates itself from __INITIAL_CONFIG__)
    if let Ok(cfg) = config.lock() {
        evaluate_and_apply(&cfg);
    }

    // 3. Wrap main_webview in Arc so the tray popup IPC can control it
    let main_webview = Arc::new(main_webview);

    // --- Custom Frameless HTML Popup Window ---
    let popup_window = WindowBuilder::new()
        .with_title("Tray Menu")
        .with_inner_size(LogicalSize::new(190.0, 110.0))
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_visible(false)
        .build(&event_loop)?;

    let proxy_main = Arc::clone(&main_webview);
    
    // Clone directly from `config` here, since `config` is still available in main()
    let config_ipc_popup = Arc::clone(&config);

    let popup_webview = WebViewBuilder::new(popup_window)?
        .with_transparent(true)
        .with_html(TRAY_MENU_HTML)?
        .with_ipc_handler(move |window, req| match req.as_str() {
            "TOGGLE" => {
                let vis = proxy_main.window().is_visible();
                let next_vis = !vis;
                proxy_main.window().set_visible(next_vis);

                if next_vis {
                    proxy_main.window().set_focus();

                    if let Ok(cfg) = config_ipc_popup.lock() {
                        if let Ok(json_str) = serde_json::to_string(&*cfg) {
                            let script = format!("if (window.hydrateConfig) window.hydrateConfig({});", json_str);
                            let _ = proxy_main.evaluate_script(&script);
                        }
                    }
                }
                window.set_visible(false);
            }
            "QUIT" => {
                // Hand the taskbar back to Windows before leaving
                apply_taskbar_state(ACCENT_DISABLED, 0x00000000, true);
                std::process::exit(0);
            }
            _ => {}
        })
        .build()?;

    let popup_window_id = popup_webview.window().id();
    let main_window_id = main_webview.window().id();

    let mut last_popup_show = Instant::now();

    // Event Loop
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            // Handle System Tray Clicks
            Event::TrayEvent { .. } => {
                let now = Instant::now();
                let is_visible = popup_webview.window().is_visible();

                // If visible and opened over 300ms ago, toggle it off on click
                if is_visible && now.duration_since(last_popup_show).as_millis() > 300 {
                    popup_webview.window().set_visible(false);
                } else if !is_visible {
                    unsafe {
                        let mut pt: POINT = std::mem::zeroed();
                        GetCursorPos(&mut pt);

                        let menu_w = 190;
                        let menu_h = 110;
                        let x = pt.x - menu_w + 10;
                        let y = pt.y - menu_h - 10;

                        let target_x = if x < 0 { pt.x } else { x };
                        let target_y = if y < 0 { pt.y } else { y };

                        popup_webview
                            .window()
                            .set_outer_position(PhysicalPosition::new(target_x, target_y));
                        popup_webview.window().set_visible(true);
                        popup_webview.window().set_focus();
                        last_popup_show = Instant::now();
                    }
                }
            }

            // Auto-hide tray menu when clicking outside (loss of focus)
            Event::WindowEvent {
                window_id,
                event: WindowEvent::Focused(false),
                ..
            } => {
                if window_id == popup_window_id {
                    // Ignore Windows taskbar focus-stealing immediately after launch (250ms grace period)
                    if last_popup_show.elapsed().as_millis() > 250 {
                        popup_webview.window().set_visible(false);
                    }
                }
            }

            // Hide main panel on close instead of exiting app
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if window_id == main_window_id {
                    main_webview.window().set_visible(false);
                }
            }

            _ => {}
        }
    });
}