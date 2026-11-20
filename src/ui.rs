//! Win32 front end: hidden window, global hotkey, tray icon, GDI painting.

use crate::search::{self, Entry, Searcher};
use crate::textedit::LineEdit;
use crate::config::{self, Settings, Theme};
use crate::{apps, calc, index, startup, update, web};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use std::time::Duration;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_INDEXED: u32 = WM_APP + 2;
const WM_UPDATED: u32 = WM_APP + 3;
const HOTKEY_ID: i32 = 1;
const ID_EXIT: usize = 1001;
const ID_UPDATE: usize = 1002;
const ID_STARTUP: usize = 1003;
const ID_HOTKEY_BASE: usize = 1100; // + preset index
const ID_HOTKEY_CUSTOM: usize = 1199;
const ID_THEME_SYSTEM: usize = 1200;
const ID_THEME_DARK: usize = 1201;
const ID_THEME_LIGHT: usize = 1202;
const TIMER_CARET: usize = 1;
const RESCAN_AFTER: Duration = Duration::from_secs(30);
const AFTER_UPDATE_ARG: &str = "--after-update";
const MAX_RESULTS: usize = 8;

// Layout in 96-dpi pixels.
const WIDTH: i32 = 640;
const INPUT_H: i32 = 60;
const ROW_H: i32 = 44;
const PAD: i32 = 18;

// Colors are 0x00BBGGRR.
#[derive(Clone, Copy)]
struct Palette {
    bg: u32,
    bg_selected: u32,
    fg: u32,
    fg_dim: u32,
    selection: u32,
}

const DARK: Palette = Palette {
    bg: 0x00261F1C,
    bg_selected: 0x00453A33,
    fg: 0x00F2EDE9,
    fg_dim: 0x009A8E86,
    selection: 0x00A0542B,
};

const LIGHT: Palette = Palette {
    bg: 0x00FAFAFA,
    bg_selected: 0x00F0E8E4,
    fg: 0x00231F1F,
    fg_dim: 0x007A6F6B,
    selection: 0x00F5D2B9,
};

const ICON_SIZE: i32 = 24;
// Segoe MDL2 Assets glyphs (built into Windows 10/11).
const GLYPH_CALC: char = '\u{E8EF}';
const GLYPH_SEARCH: char = '\u{E721}';

static INDEX: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static HINST: OnceLock<isize> = OnceLock::new();
static SCANNING: AtomicBool = AtomicBool::new(false);
static LAST_SCAN: Mutex<Option<Instant>> = Mutex::new(None);

thread_local! {
    static MUTEX: Cell<Option<HANDLE>> = const { Cell::new(None) };
    static PENDING_RESTART: Cell<bool> = const { Cell::new(false) };
    static CARET_ON: Cell<bool> = const { Cell::new(true) };
    static PALETTE: Cell<Palette> = const { Cell::new(DARK) };
    static SETTINGS: RefCell<Settings> = RefCell::new(Settings::default());
    static SETTINGS_MTIME: Cell<Option<std::time::SystemTime>> = const { Cell::new(None) };
}

/// One row in the results list.
#[derive(Clone)]
enum Item {
    App(Entry),
    Calc(String),
    Web(String),
}

struct State {
    edit: LineEdit,
    results: Vec<Item>,
    selected: usize,
    usage: HashMap<String, u32>,
    searcher: Searcher,
    scale: f32,
    font: HFONT,
    font_small: HFONT,
    font_icon: HFONT,
    icons: HashMap<String, HICON>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn px(v: i32, scale: f32) -> i32 {
    (v as f32 * scale).round() as i32
}

fn make_font(height: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -height, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI"),
        )
    }
}

pub fn run() {
    unsafe {
        let after_update = std::env::args().any(|a| a == AFTER_UPDATE_ARG);

        // Single instance. Right after a self-update the old process may still
        // be shutting down, so wait for it briefly.
        let mut tries = if after_update { 50 } else { 1 };
        loop {
            let m = CreateMutexW(None, true, w!("Local\\DartLauncherSingleton"));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                MUTEX.with(|c| c.set(m.ok()));
                break;
            }
            if let Ok(h) = m {
                let _ = CloseHandle(h);
            }
            tries -= 1;
            if tries == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // for shell icons
        update::cleanup();

        let hmod = GetModuleHandleW(None).unwrap();
        let hinst = HINSTANCE(hmod.0);
        let _ = HINST.set(hinst.0 as isize);
        startup::refresh();
        let class = w!("DartLauncherWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class,
            w!("Dart"),
            WS_POPUP,
            0, 0, 10, 10,
            None, None, Some(hinst), None,
        )
        .expect("create window");

        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                edit: LineEdit::default(),
                results: Vec::new(),
                selected: 0,
                usage: search::load_usage(),
                searcher: Searcher::new(),
                scale: 1.0,
                font: make_font(20, 400),
                font_small: make_font(13, 400),
                font_icon: CreateFontW(
                    -20, 0, 0, 0, 400, 0, 0, 0,
                    DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0,
                    w!("Segoe MDL2 Assets"),
                ),
                icons: HashMap::new(),
            });
        });

        SETTINGS.with(|c| *c.borrow_mut() = config::load());
        SETTINGS_MTIME.with(|c| c.set(config::modified()));
        apply_theme();

        // Retry briefly: after a self-update the old process may still hold it.
        let hotkey = current_hotkey();
        let mut registered = false;
        for _ in 0..30 {
            if register_hotkey(hwnd, &hotkey) {
                registered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !registered {
            let msg = format!(
                "Could not register {hotkey}. Another program is using it.\n\n\
                 Right-click the Dart tray icon and pick a different hotkey."
            );
            MessageBoxW(None, &HSTRING::from(msg), w!("Dart"), MB_OK | MB_ICONWARNING);
        }

        add_tray_icon(hwnd);
        if after_update {
            notify(hwnd, "Dart updated", &format!("Now running version {}.", update::CURRENT));
        }

        // Index in the background.
        let h = hwnd.0 as isize;
        start_scan(hwnd);

        // Auto-update: shortly after start, then every 6 hours.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(20));
            loop {
                if let Ok(update::Outcome::Updated(_)) = update::check_and_apply() {
                    let _ = PostMessageW(Some(HWND(h as *mut _)), WM_UPDATED, WPARAM(0), LPARAM(0));
                    return;
                }
                std::thread::sleep(Duration::from_secs(6 * 3600));
            }
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        remove_tray_icon(hwnd);
    }
}

/// Rescans apps on a background thread (no-op if a scan is already running).
fn start_scan(hwnd: HWND) {
    if SCANNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let mut entries = index::scan();
        let mut seen: std::collections::HashSet<String> =
            entries.iter().map(|e| e.name.to_lowercase()).collect();
        for app in apps::scan() {
            if seen.insert(app.name.to_lowercase()) {
                entries.push(app);
            }
        }
        entries.sort_by_key(|e| e.name.to_lowercase());
        *INDEX.lock().unwrap() = entries;
        *LAST_SCAN.lock().unwrap() = Some(Instant::now());
        SCANNING.store(false, Ordering::SeqCst);
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_INDEXED, WPARAM(0), LPARAM(0));
        }
    });
}

/// Rescans if the last scan is old, so newly installed apps show up.
fn rescan_if_stale(hwnd: HWND) {
    let stale = LAST_SCAN.lock().unwrap().is_none_or(|t| t.elapsed() > RESCAN_AFTER);
    if stale {
        start_scan(hwnd);
    }
}

unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_TIP | NIF_MESSAGE,
        uCallbackMessage: WM_TRAY,
        hIcon: app_icon(GetSystemMetrics(SM_CXSMICON)),
        ..Default::default()
    };
    let tip: Vec<u16> = format!("Dart {} ({})", update::CURRENT, current_hotkey()).encode_utf16().collect();
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    nid
}

/// Loads the embedded app icon (resource ID 1) at the given size.
unsafe fn app_icon(size: i32) -> HICON {
    let hinst = HINST.get().map(|&h| HINSTANCE(h as *mut _));
    match LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
        Ok(h) if !h.is_invalid() => HICON(h.0),
        _ => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
    }
}

/// Shows a tray notification.
unsafe fn notify(hwnd: HWND, title: &str, text: &str) {
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    let t: Vec<u16> = title.encode_utf16().collect();
    let n = t.len().min(63);
    nid.szInfoTitle[..n].copy_from_slice(&t[..n]);
    let b: Vec<u16> = text.encode_utf16().collect();
    let n = b.len().min(255);
    nid.szInfo[..n].copy_from_slice(&b[..n]);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Starts the freshly swapped-in exe and exits this one.
unsafe fn restart(hwnd: HWND) {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
    remove_tray_icon(hwnd);
    if let Some(m) = MUTEX.with(|c| c.take()) {
        let _ = CloseHandle(m);
    }
    if std::process::Command::new(exe).arg(AFTER_UPDATE_ARG).spawn().is_err() {
        // Could not start the new version; keep running this one.
        add_tray_icon(hwnd);
        register_hotkey(hwnd, &current_hotkey());
        return;
    }
    let _ = DestroyWindow(hwnd);
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(hwnd));
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
}

// --------------------------------------------------------------- settings

fn current_hotkey() -> String {
    SETTINGS.with(|c| c.borrow().hotkey.clone())
}

/// (Re)registers the global hotkey. Returns false if it's taken or invalid.
unsafe fn register_hotkey(hwnd: HWND, hotkey: &str) -> bool {
    let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
    let Some((mods, vk)) = config::parse_hotkey(hotkey) else { return false };
    RegisterHotKey(Some(hwnd), HOTKEY_ID, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk).is_ok()
}

/// Reads the Windows "apps use light theme" setting.
fn system_uses_light_theme() -> bool {
    use windows::Win32::System::Registry::*;
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    rc == ERROR_SUCCESS && value != 0
}

fn apply_theme() {
    let theme = SETTINGS.with(|c| c.borrow().theme);
    let light = match theme {
        Theme::Light => true,
        Theme::Dark => false,
        Theme::System => system_uses_light_theme(),
    };
    PALETTE.with(|c| c.set(if light { LIGHT } else { DARK }));
}

fn palette() -> Palette {
    PALETTE.with(|c| c.get())
}

/// Applies new settings: re-registers the hotkey (keeping the old one if the
/// new one is taken), switches theme, saves, and refreshes the tray tooltip.
unsafe fn apply_settings(hwnd: HWND, mut new: Settings) {
    let old = SETTINGS.with(|c| c.borrow().clone());
    if !new.hotkey.eq_ignore_ascii_case(&old.hotkey) && !register_hotkey(hwnd, &new.hotkey) {
        let msg = format!("{} is already used by another program. Keeping {}.", new.hotkey, old.hotkey);
        MessageBoxW(Some(hwnd), &HSTRING::from(msg), w!("Dart"), MB_OK | MB_ICONWARNING);
        register_hotkey(hwnd, &old.hotkey);
        new.hotkey = old.hotkey.clone();
    }
    SETTINGS.with(|c| *c.borrow_mut() = new.clone());
    config::save(&new);
    SETTINGS_MTIME.with(|c| c.set(config::modified()));
    apply_theme();
    let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(hwnd));
    let _ = InvalidateRect(Some(hwnd), None, false);
}

/// Picks up hand edits to settings.ini.
unsafe fn reload_settings_if_changed(hwnd: HWND) {
    let now = config::modified();
    if now.is_some() && now != SETTINGS_MTIME.with(|c| c.get()) {
        SETTINGS_MTIME.with(|c| c.set(now));
        apply_settings(hwnd, config::load());
    }
}

unsafe fn edit_settings_file() {
    let Some(path) = config::path() else { return };
    if !path.exists() {
        config::save(&SETTINGS.with(|c| c.borrow().clone()));
    }
    ShellExecuteW(
        None,
        w!("open"),
        w!("notepad.exe"),
        &HSTRING::from(format!("\"{}\"", path.display())),
        PCWSTR::null(),
        SW_SHOWNORMAL,
    );
}

unsafe fn show_tray_menu(hwnd: HWND) {
    reload_settings_if_changed(hwnd);
    let menu = CreatePopupMenu().unwrap();
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &HSTRING::from(format!("Dart {}", update::CURRENT)));
    let _ = AppendMenuW(menu, MF_STRING, ID_UPDATE, w!("Check for updates"));
    let check = if startup::is_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, MF_STRING | check, ID_STARTUP, w!("Start with Windows"));

    let settings = SETTINGS.with(|c| c.borrow().clone());
    let hk_menu = CreatePopupMenu().unwrap();
    let mut is_preset = false;
    for (i, p) in config::HOTKEY_PRESETS.iter().enumerate() {
        let on = p.eq_ignore_ascii_case(&settings.hotkey);
        is_preset |= on;
        let flag = if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(hk_menu, MF_STRING | flag, ID_HOTKEY_BASE + i, &HSTRING::from(*p));
    }
    if !is_preset {
        let _ = AppendMenuW(hk_menu, MF_STRING | MF_CHECKED | MF_GRAYED, 0, &HSTRING::from(settings.hotkey.as_str()));
    }
    let _ = AppendMenuW(hk_menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(hk_menu, MF_STRING, ID_HOTKEY_CUSTOM, w!("Custom… (edit settings file)"));
    let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, hk_menu.0 as usize, w!("Hotkey"));

    let th_menu = CreatePopupMenu().unwrap();
    for (id, t, label) in [
        (ID_THEME_SYSTEM, Theme::System, w!("Match Windows")),
        (ID_THEME_DARK, Theme::Dark, w!("Dark")),
        (ID_THEME_LIGHT, Theme::Light, w!("Light")),
    ] {
        let flag = if settings.theme == t { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(th_menu, MF_STRING | flag, id, label);
    }
    let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, th_menu.0 as usize, w!("Theme"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd); // required so the menu closes properly
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, Some(0), hwnd, None);
    let _ = DestroyMenu(menu);
}

fn manual_update_check(hwnd: HWND) {
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let msg = match update::check_and_apply() {
            Ok(update::Outcome::UpToDate) => "Dart is up to date.".to_string(),
            Ok(update::Outcome::Updated(_)) => {
                // Restarts straight away; the new version shows a notification.
                unsafe {
                    let _ = PostMessageW(Some(HWND(h as *mut _)), WM_UPDATED, WPARAM(0), LPARAM(0));
                }
                return;
            }
            Err(e) => format!("Update check failed: {e}"),
        };
        unsafe {
            MessageBoxW(None, &HSTRING::from(msg), w!("Dart"), MB_OK | MB_ICONINFORMATION);
        }
    });
}

// ---------------------------------------------------------------- showing

unsafe fn show(hwnd: HWND) {
    reload_settings_if_changed(hwnd);
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let work = mi.rcWork;

    let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
    let scale = dpi as f32 / 96.0;

    with_state(|s| {
        s.scale = scale;
        s.edit.clear();
        s.selected = 0;
        refresh(s);
    });

    let (w_px, h_px) = window_size(hwnd);
    let x = work.left + ((work.right - work.left) - w_px) / 2;
    let y = work.top + ((work.bottom - work.top) as f32 * 0.2) as i32;
    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w_px, h_px, SWP_SHOWWINDOW);
    let rgn = CreateRoundRectRgn(0, 0, w_px + 1, h_px + 1, px(16, scale), px(16, scale));
    SetWindowRgn(hwnd, Some(rgn), true);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    CARET_ON.with(|c| c.set(true));
    SetTimer(Some(hwnd), TIMER_CARET, GetCaretBlinkTime().clamp(200, 1200), None);
    let _ = InvalidateRect(Some(hwnd), None, false);
    rescan_if_stale(hwnd);
}

unsafe fn hide(hwnd: HWND) {
    let _ = ShowWindow(hwnd, SW_HIDE);
    let _ = KillTimer(Some(hwnd), TIMER_CARET);
    if PENDING_RESTART.with(|c| c.replace(false)) {
        // Let the app launch (if any) go first, then restart.
        let _ = PostMessageW(Some(hwnd), WM_UPDATED, WPARAM(0), LPARAM(0));
    }
}

fn window_size(_hwnd: HWND) -> (i32, i32) {
    with_state(|s| {
        let rows = s.results.len() as i32;
        let h = INPUT_H + if rows > 0 { rows * ROW_H + 8 } else { 0 };
        (px(WIDTH, s.scale), px(h, s.scale))
    })
    .unwrap_or((WIDTH, INPUT_H))
}

fn refresh(s: &mut State) {
    s.results.clear();
    let query = s.edit.text();
    if let Some(terms) = web::explicit(&query) {
        s.results.push(Item::Web(terms.to_string()));
    } else {
        if let Some(v) = calc::eval(&query) {
            s.results.push(Item::Calc(v));
        }
        let index = INDEX.lock().unwrap();
        let room = MAX_RESULTS - s.results.len();
        let hits = s.searcher.search(&index, &s.usage, &query, room);
        s.results.extend(hits.into_iter().map(|i| Item::App(index[i].clone())));
        if s.results.is_empty() && !query.trim().is_empty() {
            s.results.push(Item::Web(query.trim().to_string()));
        }
    }
    if s.selected >= s.results.len() {
        s.selected = 0;
    }
}

unsafe fn copy_to_clipboard(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    if OpenClipboard(Some(hwnd)).is_err() {
        return;
    }
    let _ = EmptyClipboard();
    if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(h);
            // On success the clipboard owns the memory.
            let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0)));
        }
    }
    let _ = CloseClipboard();
}

unsafe fn read_clipboard(hwnd: HWND) -> Option<String> {
    OpenClipboard(Some(hwnd)).ok()?;
    let mut out = None;
    if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
        let g = windows::Win32::Foundation::HGLOBAL(h.0);
        let p = GlobalLock(g) as *const u16;
        if !p.is_null() {
            let mut len = 0;
            while *p.add(len) != 0 && len < 4096 {
                len += 1;
            }
            out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
            let _ = GlobalUnlock(g);
        }
    }
    let _ = CloseClipboard();
    out
}

/// Loads (and caches) the shell icon for a path or shell: URL.
unsafe fn icon_for(s: &mut State, path: &str) -> HICON {
    if let Some(&h) = s.icons.get(path) {
        return h;
    }
    let mut icon = HICON::default();
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    if SHParseDisplayName(&HSTRING::from(path), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
        let mut sfi = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            PCWSTR(pidl as *const u16),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut sfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ok != 0 {
            icon = sfi.hIcon;
        }
        ILFree(Some(pidl));
    }
    s.icons.insert(path.to_string(), icon); // cache failures too
    icon
}

unsafe fn resize_to_fit(hwnd: HWND) {
    let (w_px, h_px) = window_size(hwnd);
    let mut r = RECT::default();
    let _ = GetWindowRect(hwnd, &mut r);
    let _ = SetWindowPos(hwnd, None, r.left, r.top, w_px, h_px, SWP_NOZORDER | SWP_NOACTIVATE);
    let scale = with_state(|s| s.scale).unwrap_or(1.0);
    let rgn = CreateRoundRectRgn(0, 0, w_px + 1, h_px + 1, px(16, scale), px(16, scale));
    SetWindowRgn(hwnd, Some(rgn), true);
    let _ = InvalidateRect(Some(hwnd), None, false);
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Open,
    /// Ctrl+Enter: show the file in Explorer.
    Location,
    /// Ctrl+Shift+Enter: run as administrator.
    Admin,
}

unsafe fn launch_selected(hwnd: HWND, mode: Mode) {
    let item = with_state(|s| s.results.get(s.selected).cloned()).flatten();
    let target = match item {
        None => return,
        Some(Item::Calc(v)) => {
            hide(hwnd);
            copy_to_clipboard(hwnd, &v);
            return;
        }
        Some(Item::Web(terms)) => web::url(&terms),
        Some(Item::App(entry)) => {
            let store_app = entry.path.starts_with("shell:");
            if mode == Mode::Location && store_app {
                return; // Store apps have no file to show; keep the window open.
            }
            if mode != Mode::Location {
                with_state(|s| {
                    *s.usage.entry(entry.path.clone()).or_insert(0) += 1;
                    search::save_usage(&s.usage);
                });
            }
            if mode == Mode::Location {
                hide(hwnd);
                let args = format!("/select,\"{}\"", entry.path);
                ShellExecuteW(None, w!("open"), w!("explorer.exe"), &HSTRING::from(args), PCWSTR::null(), SW_SHOWNORMAL);
                return;
            }
            if mode == Mode::Admin && !store_app {
                hide(hwnd);
                ShellExecuteW(None, w!("runas"), &HSTRING::from(entry.path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                return;
            }
            entry.path
        }
    };
    hide(hwnd);
    ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
}

/// Call after the query text changed.
unsafe fn edited(hwnd: HWND) {
    with_state(|s| {
        s.selected = 0;
        refresh(s);
    });
    CARET_ON.with(|c| c.set(true));
    resize_to_fit(hwnd);
}

/// Call after the caret moved without changing the text.
unsafe fn caret_moved(hwnd: HWND) {
    CARET_ON.with(|c| c.set(true));
    let _ = InvalidateRect(Some(hwnd), None, false);
}

// --------------------------------------------------------------- painting

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right - rc.left, rc.bottom - rc.top);

    // Double buffer.
    let mem = CreateCompatibleDC(Some(hdc));
    let bmp = CreateCompatibleBitmap(hdc, cw, ch);
    let old = SelectObject(mem, bmp.into());

    with_state(|s| {
        let sc = s.scale;
        let pal = palette();
        let bg = CreateSolidBrush(COLORREF(pal.bg));
        FillRect(mem, &rc, bg);
        let _ = DeleteObject(bg.into());
        SetBkMode(mem, TRANSPARENT);

        // Input line.
        SelectObject(mem, s.font.into());
        let mut input_rc = RECT { left: px(PAD, sc), top: 0, right: cw - px(PAD, sc), bottom: px(INPUT_H, sc) };
        let query = s.edit.text();
        let (text, color) = if query.is_empty() {
            ("Type to search…".to_string(), pal.fg_dim)
        } else {
            (query.clone(), pal.fg)
        };
        let text_w = |t: &str| -> i32 {
            let w: Vec<u16> = t.encode_utf16().collect();
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(mem, &w, &mut sz);
            sz.cx
        };
        let mid = px(INPUT_H, sc) / 2;
        let half = px(13, sc);
        if s.edit.all_selected() {
            let sel = RECT { left: input_rc.left, top: mid - half, right: input_rc.left + text_w(&query), bottom: mid + half };
            let b = CreateSolidBrush(COLORREF(pal.selection));
            FillRect(mem, &sel, b);
            let _ = DeleteObject(b.into());
        }
        SetTextColor(mem, COLORREF(color));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(mem, &mut wide, &mut input_rc, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        if CARET_ON.with(|c| c.get()) && !s.edit.all_selected() {
            let x = input_rc.left + text_w(&s.edit.before_caret());
            let caret = RECT { left: x, top: mid - half, right: x + px(2, sc).max(1), bottom: mid + half };
            let b = CreateSolidBrush(COLORREF(pal.fg));
            FillRect(mem, &caret, b);
            let _ = DeleteObject(b.into());
        }

        // Results.
        let rows = s.results.clone();
        for (row, item) in rows.into_iter().enumerate() {
            let (title, detail) = match &item {
                Item::App(e) => {
                    let detail = if e.path.starts_with("shell:AppsFolder") { "Store app".to_string() } else { e.path.clone() };
                    (e.name.clone(), detail)
                }
                Item::Calc(v) => (format!("= {v}"), "Enter to copy".to_string()),
                Item::Web(t) => (format!("Search the web for \u{201C}{t}\u{201D}"), "Google".to_string()),
            };
            let top = px(INPUT_H, sc) + 4 + row as i32 * px(ROW_H, sc);
            let row_rc = RECT { left: px(8, sc), top, right: cw - px(8, sc), bottom: top + px(ROW_H, sc) };
            if row == s.selected {
                let b = CreateSolidBrush(COLORREF(pal.bg_selected));
                let rgn = CreateRoundRectRgn(row_rc.left, row_rc.top, row_rc.right, row_rc.bottom, px(10, sc), px(10, sc));
                let _ = FillRgn(mem, rgn, b);
                let _ = DeleteObject(rgn.into());
                let _ = DeleteObject(b.into());
            }
            // Icon column.
            let icon_sz = px(ICON_SIZE, sc);
            let icon_x = px(PAD, sc);
            let icon_y = row_rc.top + (row_rc.bottom - row_rc.top - icon_sz) / 2;
            match &item {
                Item::App(e) => {
                    let h = icon_for(s, &e.path);
                    if !h.is_invalid() {
                        let _ = DrawIconEx(mem, icon_x, icon_y, h, icon_sz, icon_sz, 0, None, DI_NORMAL);
                    }
                }
                _ => {
                    let glyph = if matches!(item, Item::Calc(_)) { GLYPH_CALC } else { GLYPH_SEARCH };
                    SetTextColor(mem, COLORREF(pal.fg_dim));
                    SelectObject(mem, s.font_icon.into());
                    let mut g_rc = RECT { left: icon_x, top: row_rc.top, right: icon_x + icon_sz, bottom: row_rc.bottom };
                    let mut g: Vec<u16> = glyph.to_string().encode_utf16().collect();
                    DrawTextW(mem, &mut g, &mut g_rc, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
                }
            }

            let text_x = icon_x + icon_sz + px(12, sc);
            SetTextColor(mem, COLORREF(pal.fg));
            SelectObject(mem, s.font.into());
            let mut name_rc = RECT { left: text_x, top: row_rc.top, right: cw / 2 + px(80, sc), bottom: row_rc.bottom };
            let mut name: Vec<u16> = title.encode_utf16().collect();
            DrawTextW(mem, &mut name, &mut name_rc, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);

            SetTextColor(mem, COLORREF(pal.fg_dim));
            SelectObject(mem, s.font_small.into());
            let mut path_rc = RECT { left: cw / 2 + px(90, sc), top: row_rc.top, right: cw - px(PAD, sc), bottom: row_rc.bottom };
            let mut path: Vec<u16> = detail.encode_utf16().collect();
            DrawTextW(mem, &mut path, &mut path_rc, DT_SINGLELINE | DT_VCENTER | DT_PATH_ELLIPSIS | DT_NOPREFIX | DT_RIGHT);
        }
    });

    let _ = BitBlt(hdc, 0, 0, cw, ch, Some(mem), 0, 0, SRCCOPY);
    SelectObject(mem, old);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

/// Maps a mouse position (from a WM_MOUSE* lParam) to a result row.
fn row_at(lp: LPARAM) -> Option<usize> {
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    with_state(|s| {
        let top = px(INPUT_H, s.scale) + 4;
        if y < top {
            return None;
        }
        let row = ((y - top) / px(ROW_H, s.scale)) as usize;
        (row < s.results.len()).then_some(row)
    })
    .flatten()
}

// --------------------------------------------------------------- messages

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if IsWindowVisible(hwnd).as_bool() {
                hide(hwnd);
            } else {
                show(hwnd);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                hide(hwnd);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_CHAR => {
            let ch = wp.0 as u32;
            match ch {
                0x01 => {
                    // Ctrl+A
                    with_state(|s| s.edit.select_all());
                    caret_moved(hwnd);
                }
                0x03 => {
                    // Ctrl+C copies the query
                    let q = with_state(|s| s.edit.text()).unwrap_or_default();
                    if !q.is_empty() {
                        copy_to_clipboard(hwnd, &q);
                    }
                }
                0x16 => {
                    // Ctrl+V
                    if let Some(t) = read_clipboard(hwnd) {
                        with_state(|s| s.edit.insert(&t));
                        edited(hwnd);
                    }
                }
                0x08 => {
                    with_state(|s| s.edit.backspace());
                    edited(hwnd);
                }
                0x7F => {
                    // Ctrl+Backspace
                    with_state(|s| s.edit.delete_word_back());
                    edited(hwnd);
                }
                c if c >= 0x20 => {
                    if let Some(c) = char::from_u32(c) {
                        with_state(|s| s.edit.insert(&c.to_string()));
                        edited(hwnd);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wp.0 == TIMER_CARET {
                CARET_ON.with(|c| c.set(!c.get()));
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            match vk {
                VK_ESCAPE => hide(hwnd),
                VK_RETURN => {
                    let mode = match (ctrl, shift) {
                        (true, true) => Mode::Admin,
                        (true, false) => Mode::Location,
                        _ => Mode::Open,
                    };
                    launch_selected(hwnd, mode);
                }
                VK_LEFT => {
                    with_state(|s| if ctrl { s.edit.word_left() } else { s.edit.left() });
                    caret_moved(hwnd);
                }
                VK_RIGHT => {
                    with_state(|s| if ctrl { s.edit.word_right() } else { s.edit.right() });
                    caret_moved(hwnd);
                }
                VK_HOME => {
                    with_state(|s| s.edit.home());
                    caret_moved(hwnd);
                }
                VK_END => {
                    with_state(|s| s.edit.end());
                    caret_moved(hwnd);
                }
                VK_DELETE => {
                    with_state(|s| s.edit.delete());
                    edited(hwnd);
                }
                VK_DOWN | VK_UP => {
                    with_state(|s| {
                        let n = s.results.len();
                        if n > 0 {
                            s.selected = if vk == VK_DOWN { (s.selected + 1) % n } else { (s.selected + n - 1) % n };
                        }
                    });
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(row) = row_at(lp) {
                let changed = with_state(|s| {
                    let c = s.selected != row;
                    s.selected = row;
                    c
                })
                .unwrap_or(false);
                if changed {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(row) = row_at(lp) {
                with_state(|s| s.selected = row);
                launch_selected(hwnd, Mode::Open);
            }
            LRESULT(0)
        }
        WM_UPDATED => {
            if IsWindowVisible(hwnd).as_bool() {
                PENDING_RESTART.with(|c| c.set(true)); // restart once it's hidden
            } else {
                restart(hwnd);
            }
            LRESULT(0)
        }
        WM_INDEXED => {
            with_state(refresh);
            if IsWindowVisible(hwnd).as_bool() {
                resize_to_fit(hwnd);
            }
            LRESULT(0)
        }
        WM_TRAY => {
            let event = (lp.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP {
                show_tray_menu(hwnd);
            } else if event == WM_LBUTTONUP {
                show(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            match wp.0 & 0xFFFF {
                ID_EXIT => {
                    PostQuitMessage(0);
                }
                ID_UPDATE => manual_update_check(hwnd),
                ID_STARTUP => startup::set(!startup::is_enabled()),
                ID_HOTKEY_CUSTOM => edit_settings_file(),
                id @ ID_THEME_SYSTEM..=ID_THEME_LIGHT => {
                    let theme = match id {
                        ID_THEME_DARK => Theme::Dark,
                        ID_THEME_LIGHT => Theme::Light,
                        _ => Theme::System,
                    };
                    let mut s = SETTINGS.with(|c| c.borrow().clone());
                    s.theme = theme;
                    apply_settings(hwnd, s);
                }
                id if (ID_HOTKEY_BASE..ID_HOTKEY_BASE + config::HOTKEY_PRESETS.len()).contains(&id) => {
                    let mut s = SETTINGS.with(|c| c.borrow().clone());
                    s.hotkey = config::HOTKEY_PRESETS[id - ID_HOTKEY_BASE].to_string();
                    apply_settings(hwnd, s);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            // Windows switched light/dark mode.
            let p = lp.0 as *const u16;
            if !p.is_null() && PCWSTR(p).to_string().is_ok_and(|s| s == "ImmersiveColorSet") {
                apply_theme();
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
