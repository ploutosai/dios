#![allow(dead_code)]

use std::sync::mpsc;

#[derive(Default)]
pub(crate) struct DroppedFiles {
    pub paths: Vec<std::path::PathBuf>,
    pub bytes: Vec<Vec<u8>>,
}
pub(crate) struct NativeDisplayData {
    pub screen_width: i32,
    pub screen_height: i32,
    pub screen_position: (u32, u32),
    pub dpi_scale: f32,
    pub high_dpi: bool,
    pub quit_requested: bool,
    pub quit_ordered: bool,
    pub native_requests: mpsc::Sender<Request>,
    pub wake_event_loop: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    pub clipboard: Box<dyn Clipboard>,
    pub dropped_files: DroppedFiles,
    pub blocking_event_loop: bool,
    pub crate_b: bool,

    #[cfg(target_vendor = "apple")]
    pub view: crate::native::apple::frameworks::ObjcId,
    #[cfg(target_os = "ios")]
    pub view_ctrl: crate::native::apple::frameworks::ObjcId,
    #[cfg(target_vendor = "apple")]
    pub gfx_api: crate::conf::AppleGfxApi,
}
#[cfg(target_vendor = "apple")]
unsafe impl Send for NativeDisplayData {}
#[cfg(target_vendor = "apple")]
unsafe impl Sync for NativeDisplayData {}

impl NativeDisplayData {
    pub fn new(
        screen_width: i32,
        screen_height: i32,
        native_requests: mpsc::Sender<Request>,
        clipboard: Box<dyn Clipboard>,
    ) -> NativeDisplayData {
        NativeDisplayData {
            screen_width,
            screen_height,
            screen_position: (0, 0),
            dpi_scale: 1.,
            high_dpi: false,
            quit_requested: false,
            quit_ordered: false,
            native_requests,
            wake_event_loop: None,
            clipboard,
            dropped_files: Default::default(),
            blocking_event_loop: false,
            crate_b: false,
            #[cfg(target_vendor = "apple")]
            gfx_api: crate::conf::AppleGfxApi::OpenGl,
            #[cfg(target_vendor = "apple")]
            view: std::ptr::null_mut(),
            #[cfg(target_os = "ios")]
            view_ctrl: std::ptr::null_mut(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum Request {
    ScheduleUpdate,
    SetCursorGrab(bool),
    ShowMouse(bool),
    SetMouseCursor(crate::CursorIcon),
    SetWindowSize { new_width: u32, new_height: u32 },
    SetWindowPosition { new_x: u32, new_y: u32 },
    SetFullscreen(bool),
    ShowKeyboard(bool),
}

#[cfg(target_os = "linux")]
pub(crate) fn make_event_loop_waker() -> std::io::Result<(
    libc::c_int,
    std::sync::Arc<dyn Fn() + Send + Sync>,
)> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }

    let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        let value: u64 = 1;
        unsafe {
            let _ = libc::write(
                fd,
                &value as *const u64 as *const libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    });

    Ok((fd, wake))
}

#[cfg(target_os = "linux")]
pub(crate) unsafe fn drain_event_loop_waker(fd: libc::c_int) {
    let mut value: u64 = 0;
    let _ = libc::read(
        fd,
        &mut value as *mut u64 as *mut libc::c_void,
        std::mem::size_of::<u64>(),
    );
}

pub trait Clipboard: Send + Sync {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, string: &str);
}

pub mod module;

#[cfg(target_os = "linux")]
pub mod linux_x11;

#[cfg(target_os = "linux")]
pub mod linux_wayland;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub mod apple;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "ios")]
pub mod ios;

#[cfg(any(target_os = "android", target_os = "linux"))]
pub mod egl;

// there is no glGetProcAddr on webgl, so its impossible to make "gl" module work
// on macos.. well, there is, but way easier to just statically link to gl
#[cfg(not(target_arch = "wasm32"))]
pub mod gl;

#[cfg(target_arch = "wasm32")]
pub use wasm::webgl as gl;

pub mod query_stab;
