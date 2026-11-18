use crate::{QuotaPanel, quota::Snapshot};
use serde::{Deserialize, Serialize};
use slint::platform::software_renderer::PremultipliedRgbaColor;
use std::{cell::Cell, ffi::c_void, mem::{size_of, zeroed}, path::PathBuf, ptr::{null, null_mut}};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Registry::*, Threading::{CreateMutexW, GetCurrentProcessId}},
    UI::{HiDpi::*, Input::KeyboardAndMouse::{SetCapture, ReleaseCapture}, WindowsAndMessaging::*}};

fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
pub fn settings_path() -> PathBuf { PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("CodexQuota/settings-rust.json") }
#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub offset_dip: Option<f64>,
    pub remaining: bool,
    #[serde(default)]
    pub pixel_icon: bool,
}
impl Default for Settings { fn default() -> Self { Self { offset_dip: None, remaining: true, pixel_icon: false } } }
impl Settings {
    pub fn load() -> Self { std::fs::read(settings_path()).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_default() }
    pub fn save(&self) { let p = settings_path(); if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); } if let Ok(s) = serde_json::to_vec(self) { let _ = std::fs::write(p, s); } }
}

pub struct Instance(HANDLE);
impl Instance {
    pub fn lock() -> Option<Self> { unsafe {
        let h = CreateMutexW(null(), 0, wide("Local\\CodexQuotaTray").as_ptr());
        if h.is_null() { return None; }
        if GetLastError() == ERROR_ALREADY_EXISTS { CloseHandle(h); None } else { Some(Self(h)) }
    } }
}
impl Drop for Instance { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }

#[derive(Default)]
struct Events {
    command: Cell<u32>, pressed: Cell<bool>, dragged: Cell<bool>, press_x: Cell<i32>, origin_x: Cell<i32>,
    new_offset: Cell<Option<i32>>, save: Cell<bool>,
}
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Events;
        if !state.is_null() {
            let state = &*state;
            match message {
                WM_ERASEBKGND => return 1,
                WM_LBUTTONDOWN => {
                    let mut p: POINT = zeroed(); GetCursorPos(&mut p);
                    let mut r: RECT = zeroed(); let mut parent: RECT = zeroed();
                    GetWindowRect(hwnd, &mut r); GetWindowRect(GetParent(hwnd), &mut parent);
                    state.pressed.set(true); state.dragged.set(false); state.press_x.set(p.x); state.origin_x.set(r.left - parent.left);
                    SetCapture(hwnd); return 0;
                }
                WM_MOUSEMOVE if state.pressed.get() => {
                    let mut p: POINT = zeroed(); GetCursorPos(&mut p);
                    let delta = p.x - state.press_x.get();
                    if delta.abs() > 4 { state.dragged.set(true); }
                    if state.dragged.get() {
                        let mut parent: RECT = zeroed(); let mut r: RECT = zeroed();
                        GetWindowRect(GetParent(hwnd), &mut parent); GetWindowRect(hwnd, &mut r);
                        let x = (state.origin_x.get() + delta).clamp(0, (parent.right - parent.left - (r.right - r.left)).max(0));
                        state.new_offset.set(Some(x)); SetWindowPos(hwnd, null_mut(), x, (parent.bottom-parent.top-(r.bottom-r.top))/2, 0,0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                    return 0;
                }
                WM_LBUTTONUP if state.pressed.replace(false) => {
                    ReleaseCapture();
                    if state.dragged.get() { state.save.set(true); } else { state.command.set(100); }
                    return 0;
                }
                WM_RBUTTONUP => { state.command.set(100); return 0; }
                WM_CAPTURECHANGED if state.pressed.replace(false) => {
                    if state.dragged.replace(false) { state.save.set(true); }
                    return 0;
                }
                WM_CLOSE => { state.command.set(5); return 0; }
                _ => (),
            }
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}

pub struct Host { pub hwnd: HWND, pub parent: HWND, pub scale: f32, events: Box<Events>, pub settings: Settings }
fn owns_window(hwnd: HWND) -> bool { unsafe {
    let mut owner = 0;
    !hwnd.is_null() && IsWindow(hwnd) != 0 && GetWindowThreadProcessId(hwnd, &mut owner) != 0 && owner == GetCurrentProcessId()
} }
impl Host {
    pub fn new(settings: Settings) -> Result<Self, String> {
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
        let mut host = Self { hwnd: null_mut(), parent: null_mut(), scale: 1., events: Box::default(), settings };
        unsafe {
            let mut class: WNDCLASSW = zeroed(); class.lpfnWndProc = Some(procedure);
            class.hInstance = GetModuleHandleW(null());
            // Keep the class-name backing allocation alive until RegisterClassW returns.
            let name = wide("CodexQuotaSlintPanel"); class.lpszClassName = name.as_ptr();
            class.hCursor = LoadCursorW(null_mut(), IDC_HAND);
            RegisterClassW(&class);
        }
        host.position()?; Ok(host)
    }
    pub fn position(&mut self) -> Result<(), String> {
        unsafe {
            let parent = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
            if parent.is_null() { return Err("Explorer no tiene una barra de tareas disponible".into()); }
            let mut bar: RECT = zeroed(); GetWindowRect(parent, &mut bar);
            if bar.bottom-bar.top > bar.right-bar.left { return Err("Este adaptador necesita una barra horizontal".into()); }
            self.parent = parent; self.scale = (GetDpiForWindow(parent).max(96) as f32) / 96.;
            if !owns_window(self.hwnd) {
                self.hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    wide("CodexQuotaSlintPanel").as_ptr(), wide("Codex Quota").as_ptr(), WS_POPUP,
                    0,0,1,1,null_mut(),null_mut(),GetModuleHandleW(null()), (&*self.events as *const Events).cast_mut().cast());
                if self.hwnd.is_null() { return Err("No se pudo crear el panel nativo".into()); }
            }
            if GetParent(self.hwnd) != parent {
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, (WS_CHILD | WS_VISIBLE) as isize);
                SetParent(self.hwnd, parent);
                if GetParent(self.hwnd) != parent { return Err("Windows no permitió integrar el panel en la barra".into()); }
            }
            if self.events.pressed.get() { return Ok(()); }
            let width = (145. * self.scale).round() as i32;
            let height = (44. * self.scale).round() as i32;
            let tray = FindWindowExW(parent, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
            let mut tray_rect: RECT = zeroed();
            let end = if !tray.is_null() && GetWindowRect(tray, &mut tray_rect) != 0 { tray_rect.left-bar.left } else { bar.right-bar.left-(320. * self.scale) as i32 };
            let x = self.settings.offset_dip.map(|n| (n*self.scale as f64) as i32).unwrap_or(end-width-8).clamp(0,(bar.right-bar.left-width).max(0));
            let y = ((bar.bottom-bar.top-height)/2).max(0);
            if SetWindowPos(self.hwnd, HWND_TOP, x,y,width,height,SWP_NOACTIVATE | SWP_SHOWWINDOW) == 0 { return Err("No se pudo colocar el panel".into()); }
            Ok(())
        }
    }
    pub fn poll_command(&mut self) -> u32 {
        if self.events.save.replace(false) {
            if let Some(offset) = self.events.new_offset.take() { self.settings.offset_dip = Some(offset as f64/self.scale as f64); self.settings.save(); }
        }
        self.events.command.replace(0)
    }
    pub fn pump(&self) -> bool { unsafe {
        let mut msg: MSG = zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0,0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT { return false; }
            TranslateMessage(&msg); DispatchMessageW(&msg);
        } true
    } }
    pub fn wait(&self) { unsafe { MsgWaitForMultipleObjects(0,null(),0,1000,QS_ALLINPUT); } }
    pub fn reset(&mut self) { self.settings.offset_dip = None; self.settings.save(); let _ = self.position(); }
    pub fn paint(&self, pixels: &[PremultipliedRgbaColor], width: usize, height: usize) -> Result<(), String> {
        unsafe {
            let screen = GetDC(null_mut()); let memory = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = zeroed(); info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width as i32; info.bmiHeader.biHeight = -(height as i32);
            info.bmiHeader.biPlanes = 1; info.bmiHeader.biBitCount = 32; info.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = null_mut();
            let bitmap = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, null_mut(),0);
            if bitmap.is_null() || bits.is_null() { DeleteDC(memory); ReleaseDC(null_mut(), screen); return Err("No se pudo dibujar el panel".into()); }
            let output = std::slice::from_raw_parts_mut(bits.cast::<u8>(), width*height*4);
            for (dst,p) in output.chunks_exact_mut(4).zip(pixels) { dst.copy_from_slice(&[p.blue,p.green,p.red,p.alpha]); }
            let old = SelectObject(memory, bitmap);
            let source = POINT { x:0,y:0 }; let size = SIZE { cx:width as i32,cy:height as i32 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags:0,SourceConstantAlpha:255,AlphaFormat:AC_SRC_ALPHA as u8 };
            let ok = UpdateLayeredWindow(self.hwnd,screen,null(),&size,memory,&source,0,&blend,ULW_ALPHA);
            SelectObject(memory,old); DeleteObject(bitmap); DeleteDC(memory); ReleaseDC(null_mut(),screen);
            if ok == 0 { Err(format!("Windows rechazó la composición transparente ({})",GetLastError())) } else { Ok(()) }
        }
    }
    pub fn menu(&self, data: &Snapshot, stale: bool) -> u32 { unsafe {
        let menu = CreatePopupMenu();
        let mode = if self.settings.remaining { "remaining" } else { "used" };
        let rows = [format!("Codex quota {mode}{}", if stale { " · stale" } else { "" }),
            format!("5 hours: {}",data.five.as_ref().map(|q| q.label(self.settings.remaining)).unwrap_or("--".into())),
            data.five.as_ref().map(|q| q.reset_text()).unwrap_or("Sin datos de 5 horas".into()),
            format!("Weekly: {}", data.week.as_ref().map(|q| q.label(self.settings.remaining)).unwrap_or("--".into())),
            data.week.as_ref().map(|q| q.reset_text()).unwrap_or("Sin datos semanales".into())];
        for row in rows { AppendMenuW(menu,MF_STRING | MF_GRAYED,0,wide(&row).as_ptr()); }
        AppendMenuW(menu,MF_SEPARATOR,0,null());
        for (id,label) in [(1,"Refresh now"),(2,"Show remaining quota"),(3,"Start with Windows"),(7,"Pixel art icon"),(4,"Restablecer posición"),(6,"About Codex Quota"),(5,"Exit")] {
            let checked = (id == 2 && self.settings.remaining) || (id == 3 && startup_enabled()) || (id == 7 && self.settings.pixel_icon);
            AppendMenuW(menu, MF_STRING | if checked { MF_CHECKED } else { 0 }, id, wide(label).as_ptr());
        }
        let mut cursor: POINT = zeroed(); GetCursorPos(&mut cursor);
        SetForegroundWindow(self.hwnd);
        let choice = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, cursor.x,cursor.y,0,self.hwnd,null());
        DestroyMenu(menu); choice as u32
    } }
    pub fn about(&self) { unsafe {
        MessageBoxW(self.hwnd,wide("Codex Quota 2.0\nRust + Slint · Codex quota monitor\nUI built with Slint: https://slint.dev\nNo es una aplicación oficial de OpenAI.").as_ptr(),wide("About Codex Quota").as_ptr(),MB_OK | MB_ICONINFORMATION);
    } }
    pub fn report(&self) -> serde_json::Value { unsafe {
        let mut r: RECT = zeroed(); GetWindowRect(self.hwnd,&mut r);
        serde_json::json!({"embedded":GetParent(self.hwnd)==self.parent,"visible":IsWindowVisible(self.hwnd)!=0,"scale":self.scale,"bounds":{"x":r.left,"y":r.top,"width":r.right-r.left,"height":r.bottom-r.top}})
    } }
}
impl Drop for Host { fn drop(&mut self) { unsafe { if owns_window(self.hwnd) { DestroyWindow(self.hwnd); } } } }

pub fn light_theme() -> bool { unsafe {
    let mut value: u32 = 0; let mut bytes = 4;
    RegGetValueW(HKEY_CURRENT_USER,wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),wide("SystemUsesLightTheme").as_ptr(),RRF_RT_REG_DWORD,null_mut(),(&mut value as *mut u32).cast(),&mut bytes) == 0 && value != 0
} }
pub fn theme(panel: &QuotaPanel, light: bool) {
    panel.set_ink(slint::Color::from_rgb_u8(if light { 25 } else { 255 },if light { 25 } else { 255 },if light { 25 } else { 255 }));
    panel.set_subtle(slint::Color::from_rgb_u8(140,140,140));
    let gray = if light { 92 } else { 187 };
    panel.set_progress_ink(slint::Color::from_rgb_u8(gray, gray, gray));
}
fn run_key() -> Vec<u16> { wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run") }
pub fn startup_enabled() -> bool { unsafe {
    let mut bytes = 0;
    RegGetValueW(HKEY_CURRENT_USER,run_key().as_ptr(),wide("CodexQuota").as_ptr(),RRF_RT_REG_SZ,null_mut(),null_mut(),&mut bytes) == 0
} }
pub fn toggle_startup() { unsafe {
    let mut key = null_mut();
    if RegCreateKeyExW(HKEY_CURRENT_USER,run_key().as_ptr(),0,null(),0,KEY_SET_VALUE,null(),&mut key,null_mut()) != 0 { return; }
    if startup_enabled() { RegDeleteValueW(key,wide("CodexQuota").as_ptr()); }
    else if let Ok(exe) = std::env::current_exe() { let text = wide(&format!("\"{}\"",exe.display())); RegSetValueExW(key,wide("CodexQuota").as_ptr(),0,REG_SZ,text.as_ptr().cast(),(text.len()*2) as u32); }
    RegCloseKey(key);
} }
