//! Win32 front end: hidden window, global hotkey, tray icon, GDI painting.

use crate::search::{self, Entry, Searcher};
use crate::{apps, calc, index, update, web};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
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
const HOTKEY_ID: i32 = 1;
const ID_EXIT: usize = 1001;
const ID_UPDATE: usize = 1002;
const MAX_RESULTS: usize = 8;

// Layout in 96-dpi pixels.
const WIDTH: i32 = 640;
const INPUT_H: i32 = 60;
const ROW_H: i32 = 44;
const PAD: i32 = 18;

// Colors are 0x00BBGGRR.
const BG: u32 = 0x00261F1C;
const BG_SELECTED: u32 = 0x00453A33;
const FG: u32 = 0x00F2EDE9;
const FG_DIM: u32 = 0x009A8E86;

const ICON_SIZE: i32 = 24;
// Segoe MDL2 Assets glyphs (built into Windows 10/11).
const GLYPH_CALC: char = '\u{E8EF}';
const GLYPH_SEARCH: char = '\u{E721}';

static INDEX: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// One row in the results list.
#[derive(Clone)]
enum Item {
    App(usize),
    Calc(String),
    Web(String),
}

struct State {
    query: String,
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
        // Single instance.
        let _guard = CreateMutexW(None, true, w!("Local\\DartLauncherSingleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); // for shell icons
        update::cleanup();

        let hmod = GetModuleHandleW(None).unwrap();
        let hinst = HINSTANCE(hmod.0);
        let class = w!("DartLauncherWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
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
                query: String::new(),
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

        if RegisterHotKey(Some(hwnd), HOTKEY_ID, MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32).is_err() {
            MessageBoxW(
                None,
                w!("Could not register Alt+Space. Another program is using it."),
                w!("Dart"),
                MB_OK | MB_ICONWARNING,
            );
        }

        add_tray_icon(hwnd);

        // Index in the background.
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
            entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            *INDEX.lock().unwrap() = entries;
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_INDEXED, WPARAM(0), LPARAM(0));
        });

        // Auto-update: shortly after start, then every 6 hours.
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(20));
            loop {
                let _ = update::check_and_apply();
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

unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_TIP | NIF_MESSAGE,
        uCallbackMessage: WM_TRAY,
        hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        ..Default::default()
    };
    let tip: Vec<u16> = format!("Dart {} (Alt+Space)", update::CURRENT).encode_utf16().collect();
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    nid
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(hwnd));
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap();
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &HSTRING::from(format!("Dart {}", update::CURRENT)));
    let _ = AppendMenuW(menu, MF_STRING, ID_UPDATE, w!("Check for updates"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd); // required so the menu closes properly
    let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, Some(0), hwnd, None);
    let _ = DestroyMenu(menu);
}

fn manual_update_check() {
    std::thread::spawn(|| {
        let msg = match update::check_and_apply() {
            Ok(update::Outcome::UpToDate) => "Dart is up to date.".to_string(),
            Ok(update::Outcome::Updated(v)) => format!("Updated to {v}. Restart Dart to use it."),
            Err(e) => format!("Update check failed: {e}"),
        };
        unsafe {
            MessageBoxW(None, &HSTRING::from(msg), w!("Dart"), MB_OK | MB_ICONINFORMATION);
        }
    });
}

// ---------------------------------------------------------------- showing

unsafe fn show(hwnd: HWND) {
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
        s.query.clear();
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
    let _ = InvalidateRect(Some(hwnd), None, false);
}

unsafe fn hide(hwnd: HWND) {
    let _ = ShowWindow(hwnd, SW_HIDE);
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
    if let Some(terms) = web::explicit(&s.query) {
        s.results.push(Item::Web(terms.to_string()));
    } else {
        if let Some(v) = calc::eval(&s.query) {
            s.results.push(Item::Calc(v));
        }
        let index = INDEX.lock().unwrap();
        let room = MAX_RESULTS - s.results.len();
        let hits = s.searcher.search(&index, &s.usage, &s.query, room);
        s.results.extend(hits.into_iter().map(Item::App));
        if s.results.is_empty() && !s.query.trim().is_empty() {
            s.results.push(Item::Web(s.query.trim().to_string()));
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

unsafe fn launch_selected(hwnd: HWND) {
    let item = with_state(|s| s.results.get(s.selected).cloned()).flatten();
    hide(hwnd);
    let target = match item {
        None => return,
        Some(Item::Calc(v)) => {
            copy_to_clipboard(hwnd, &v);
            return;
        }
        Some(Item::Web(terms)) => web::url(&terms),
        Some(Item::App(idx)) => {
            let Some(entry) = INDEX.lock().unwrap().get(idx).cloned() else { return };
            with_state(|s| {
                *s.usage.entry(entry.path.clone()).or_insert(0) += 1;
                search::save_usage(&s.usage);
            });
            entry.path
        }
    };
    ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
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
        let bg = CreateSolidBrush(COLORREF(BG));
        FillRect(mem, &rc, bg);
        let _ = DeleteObject(bg.into());
        SetBkMode(mem, TRANSPARENT);

        // Input line.
        SelectObject(mem, s.font.into());
        let mut input_rc = RECT { left: px(PAD, sc), top: 0, right: cw - px(PAD, sc), bottom: px(INPUT_H, sc) };
        let (text, color) = if s.query.is_empty() {
            ("Type to search…".to_string(), FG_DIM)
        } else {
            (s.query.clone(), FG)
        };
        SetTextColor(mem, COLORREF(color));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(mem, &mut wide, &mut input_rc, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);

        // Results.
        let rows: Vec<(Item, Option<Entry>)> = {
            let index = INDEX.lock().unwrap();
            s.results
                .iter()
                .map(|it| match it {
                    Item::App(i) => (it.clone(), index.get(*i).cloned()),
                    _ => (it.clone(), None),
                })
                .collect()
        };
        for (row, (item, entry)) in rows.into_iter().enumerate() {
            let (title, detail) = match &item {
                Item::App(_) => {
                    let Some(e) = &entry else { continue };
                    let detail = if e.path.starts_with("shell:AppsFolder") { "Store app".to_string() } else { e.path.clone() };
                    (e.name.clone(), detail)
                }
                Item::Calc(v) => (format!("= {v}"), "Enter to copy".to_string()),
                Item::Web(t) => (format!("Search the web for \u{201C}{t}\u{201D}"), "Google".to_string()),
            };
            let top = px(INPUT_H, sc) + 4 + row as i32 * px(ROW_H, sc);
            let row_rc = RECT { left: px(8, sc), top, right: cw - px(8, sc), bottom: top + px(ROW_H, sc) };
            if row == s.selected {
                let b = CreateSolidBrush(COLORREF(BG_SELECTED));
                let rgn = CreateRoundRectRgn(row_rc.left, row_rc.top, row_rc.right, row_rc.bottom, px(10, sc), px(10, sc));
                let _ = FillRgn(mem, rgn, b);
                let _ = DeleteObject(rgn.into());
                let _ = DeleteObject(b.into());
            }
            // Icon column.
            let icon_sz = px(ICON_SIZE, sc);
            let icon_x = px(PAD, sc);
            let icon_y = row_rc.top + (row_rc.bottom - row_rc.top - icon_sz) / 2;
            match (&item, &entry) {
                (Item::App(_), Some(e)) => {
                    let h = icon_for(s, &e.path);
                    if !h.is_invalid() {
                        let _ = DrawIconEx(mem, icon_x, icon_y, h, icon_sz, icon_sz, 0, None, DI_NORMAL);
                    }
                }
                _ => {
                    let glyph = if matches!(item, Item::Calc(_)) { GLYPH_CALC } else { GLYPH_SEARCH };
                    SetTextColor(mem, COLORREF(FG_DIM));
                    SelectObject(mem, s.font_icon.into());
                    let mut g_rc = RECT { left: icon_x, top: row_rc.top, right: icon_x + icon_sz, bottom: row_rc.bottom };
                    let mut g: Vec<u16> = glyph.to_string().encode_utf16().collect();
                    DrawTextW(mem, &mut g, &mut g_rc, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
                }
            }

            let text_x = icon_x + icon_sz + px(12, sc);
            SetTextColor(mem, COLORREF(FG));
            SelectObject(mem, s.font.into());
            let mut name_rc = RECT { left: text_x, top: row_rc.top, right: cw / 2 + px(80, sc), bottom: row_rc.bottom };
            let mut name: Vec<u16> = title.encode_utf16().collect();
            DrawTextW(mem, &mut name, &mut name_rc, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);

            SetTextColor(mem, COLORREF(FG_DIM));
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
            if ch >= 0x20 && ch != 0x7F {
                if let Some(c) = char::from_u32(ch) {
                    with_state(|s| {
                        s.query.push(c);
                        s.selected = 0;
                        refresh(s);
                    });
                    resize_to_fit(hwnd);
                }
            } else if ch == 0x08 {
                with_state(|s| {
                    s.query.pop();
                    s.selected = 0;
                    refresh(s);
                });
                resize_to_fit(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            match vk {
                VK_ESCAPE => hide(hwnd),
                VK_RETURN => launch_selected(hwnd),
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
                launch_selected(hwnd);
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
                ID_UPDATE => manual_update_check(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
