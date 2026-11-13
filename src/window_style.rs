//! Window styling: rounded corners and the app icon.
//!
//! Rounded window corners. The window is borderless (the UI draws its own title bar), and Bevy's UI cannot clip child
//! backgrounds to rounded corners, so the rounding is requested from the system instead: on Windows 11 the DWM rounds
//! the corners of the window when asked with `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` (anti-aliased, with the
//! normal window shadow). On Windows 10 and other systems the request has no effect and the corners stay square.
//!
//! The window handle is found by its title (`WINDOW_TITLE`) and checked against this process id; the system retries
//! for a while because the window may not exist yet in the very first frames.

use bevy::prelude::*;

pub fn round_corners(mut done: Local<bool>, mut tries: Local<u32>) {
    let _bench = crate::benchmark::sys("sys.window_style.round_corners");
    if *done || *tries > 1000 {
        return;
    }
    *tries += 1;
    if apply() {
        *done = true;
    }
}

#[cfg(windows)]
fn apply() -> bool {
    use std::ffi::c_void;
    use std::ptr::null;

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut c_void;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
    }
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    unsafe {
        let title: Vec<u16> = crate::WINDOW_TITLE
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let hwnd = FindWindowW(null(), title.as_ptr());
        if hwnd.is_null() {
            return false; // not created yet
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != std::process::id() {
            return false; // another program's window with the same title
        }
        set_icon(hwnd);
        // The result is ignored: before Windows 11 the call fails and the corners simply stay square.
        let pref = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const u32 as *const c_void,
            4,
        );
        true
    }
}

/// Gives the window the exe's icon resource (id 1, embedded by `build.rs` from `assets/icon.ico`): the small size for
/// the taskbar/title, the big one for Alt+Tab. Loaded at the exact system sizes so the pixel art stays crisp. If the
/// Uses `PostMessageW`, never `SendMessageW`: this runs in a Bevy system, usually on a worker thread, while the main
/// thread (which owns the window) waits for the schedule; a blocking send from here deadlocks and the window stays black.
/// If the resource is missing (build without the resource compiler) the loads return null and nothing happens.
#[cfg(windows)]
unsafe fn set_icon(hwnd: *mut std::ffi::c_void) {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        fn LoadImageW(
            hinst: *mut c_void,
            name: *const u16,
            kind: u32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> *mut c_void;
        fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    const IMAGE_ICON: u32 = 1;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const SM_CXICON: i32 = 11;
    const SM_CYICON: i32 = 12;
    const SM_CXSMICON: i32 = 49;
    const SM_CYSMICON: i32 = 50;
    let hinst = GetModuleHandleW(std::ptr::null());
    let id: *const u16 = std::ptr::without_provenance(1); // MAKEINTRESOURCE(1): an integer id, not a pointer
    let small = LoadImageW(
        hinst,
        id,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        0,
    );
    let big = LoadImageW(
        hinst,
        id,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXICON),
        GetSystemMetrics(SM_CYICON),
        0,
    );
    if !small.is_null() {
        PostMessageW(hwnd, WM_SETICON, ICON_SMALL, small as isize);
    }
    if !big.is_null() {
        PostMessageW(hwnd, WM_SETICON, ICON_BIG, big as isize);
    }
}

#[cfg(not(windows))]
fn apply() -> bool {
    true
}
