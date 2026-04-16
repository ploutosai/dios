#![allow(clippy::needless_doctest_main)]

pub use subsecond_types::JumpTable;

use std::{
    backtrace,
    mem::transmute,
    panic::AssertUnwindSafe,
    sync::{atomic::AtomicPtr, Arc, Mutex},
};

pub fn call<O>(mut f: impl FnMut() -> O) -> O {
    if !cfg!(debug_assertions) {
        return f();
    }

    let mut hotfn = HotFn::current(f);
    loop {
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| hotfn.call(())));
        let err = match res {
            Ok(res) => return res,
            Err(err) => err,
        };

        let Some(_hot_payload) = err.downcast_ref::<HotFnPanic>() else {
            std::panic::resume_unwind(err);
        };
    }
}

static APP_JUMP_TABLE: AtomicPtr<JumpTable> = AtomicPtr::new(std::ptr::null_mut());
static HOTRELOAD_HANDLERS: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(Vec::new());

pub fn register_handler(handler: Arc<dyn Fn() + Send + Sync + 'static>) {
    HOTRELOAD_HANDLERS.lock().unwrap().push(handler);
}

pub unsafe fn get_jump_table() -> Option<&'static JumpTable> {
    let ptr = APP_JUMP_TABLE.load(std::sync::atomic::Ordering::Relaxed);
    if ptr.is_null() {
        return None;
    }

    Some(unsafe { &*ptr })
}

unsafe fn commit_patch(table: JumpTable) {
    APP_JUMP_TABLE.store(
        Box::into_raw(Box::new(table)),
        std::sync::atomic::Ordering::Relaxed,
    );
    HOTRELOAD_HANDLERS
        .lock()
        .unwrap()
        .clone()
        .iter()
        .for_each(|handler| {
            handler();
        });
}

#[derive(Debug)]
pub struct HotFnPanic {
    _backtrace: backtrace::Backtrace,
}

#[non_exhaustive]
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct HotFnPtr(pub u64);

impl HotFnPtr {
    pub unsafe fn new(index: u64) -> Self {
        Self(index)
    }
}

pub struct HotFn<A, M, F>
where
    F: HotFunction<A, M>,
{
    inner: F,
    _marker: std::marker::PhantomData<(A, M)>,
}

impl<A, M, F: HotFunction<A, M>> HotFn<A, M, F> {
    pub const fn current(f: F) -> HotFn<A, M, F> {
        HotFn {
            inner: f,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn call(&mut self, args: A) -> F::Return {
        self.try_call(args).unwrap()
    }

    pub fn ptr_address(&self) -> HotFnPtr {
        if size_of::<F>() == size_of::<fn() -> ()>() {
            let ptr: usize = unsafe { std::mem::transmute_copy(&self.inner) };
            return HotFnPtr(ptr as u64);
        }

        let known_fn_ptr = <F as HotFunction<A, M>>::call_it as *const () as usize;
        if let Some(jump_table) = unsafe { get_jump_table() } {
            if let Some(ptr) = jump_table.map.get(&(known_fn_ptr as u64)).cloned() {
                return HotFnPtr(ptr);
            }
        }

        HotFnPtr(known_fn_ptr as u64)
    }

    pub fn try_call(&mut self, args: A) -> Result<F::Return, HotFnPanic> {
        if !cfg!(debug_assertions) {
            return Ok(self.inner.call_it(args));
        }

        unsafe {
            if size_of::<F>() == size_of::<fn() -> ()>() {
                return Ok(self.inner.call_as_ptr(args));
            }

            if let Some(jump_table) = get_jump_table() {
                let known_fn_ptr = <F as HotFunction<A, M>>::call_it as *const () as u64;
                if let Some(ptr) = jump_table.map.get(&known_fn_ptr).cloned() {
                    let call_it = transmute::<*const (), fn(&F, A) -> F::Return>(ptr as _);
                    return Ok(call_it(&self.inner, args));
                }
            }

            Ok(self.inner.call_it(args))
        }
    }

    pub unsafe fn try_call_with_ptr(
        &mut self,
        ptr: HotFnPtr,
        args: A,
    ) -> Result<F::Return, HotFnPanic> {
        if !cfg!(debug_assertions) {
            return Ok(self.inner.call_it(args));
        }

        unsafe {
            if size_of::<F>() == size_of::<fn() -> ()>() {
                return Ok(self.inner.call_as_ptr(args));
            }

            let call_it = transmute::<*const (), fn(&F, A) -> F::Return>(ptr.0 as _);
            Ok(call_it(&self.inner, args))
        }
    }
}

pub unsafe fn apply_patch(table: JumpTable) -> Result<(), PatchError> {
    #[cfg(any(unix, windows))]
    {
        #[cfg(target_os = "android")]
        let lib = Box::leak(Box::new(android_memmap_dlopen(&table.lib)?));

        #[cfg(not(target_os = "android"))]
        let lib = Box::leak(Box::new({
            match libloading::Library::new(&table.lib) {
                Ok(lib) => lib,
                Err(err) => return Err(PatchError::Dlopen(err.to_string())),
            }
        }));

        let old_offset = aslr_reference() - table.aslr_reference as usize;

        let new_offset = unsafe {
            lib.get::<*const ()>(b"main")
                .ok()
                .unwrap()
                .try_as_raw_ptr()
                .unwrap()
                .wrapping_byte_sub(table.new_base_address as usize) as usize
        };

        let mut table = table;
        table.map = table
            .map
            .iter()
            .map(|(k, v)| {
                (
                    (*k as usize + old_offset) as u64,
                    (*v as usize + new_offset) as u64,
                )
            })
            .collect();

        commit_patch(table);
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = table;
    }

    Ok(())
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PatchError {
    #[error("Failed to load library: {0}")]
    Dlopen(String),

    #[error("Failed to load library on Android: {0}")]
    AndroidMemfd(String),
}

pub fn aslr_reference() -> usize {
    #[cfg(target_family = "wasm")]
    return 0;

    #[cfg(not(target_family = "wasm"))]
    unsafe {
        use std::ffi::c_void;

        static mut MAIN_PTR: *mut c_void = std::ptr::null_mut();

        if MAIN_PTR.is_null() {
            #[cfg(unix)]
            {
                MAIN_PTR = libc::dlsym(libc::RTLD_DEFAULT, c"main".as_ptr() as _);
            }

            #[cfg(windows)]
            {
                extern "system" {
                    fn GetModuleHandleA(lpModuleName: *const i8) -> *mut std::ffi::c_void;
                    fn GetProcAddress(
                        hModule: *mut std::ffi::c_void,
                        lpProcName: *const i8,
                    ) -> *mut std::ffi::c_void;
                }

                MAIN_PTR =
                    GetProcAddress(GetModuleHandleA(std::ptr::null()), c"main".as_ptr() as _) as _;
            }
        }

        MAIN_PTR as usize
    }
}

#[cfg(target_os = "android")]
unsafe fn android_memmap_dlopen(file: &std::path::Path) -> Result<libloading::Library, PatchError> {
    use std::ffi::{c_void, CStr, CString};
    use std::os::fd::{AsRawFd, BorrowedFd};
    use std::ptr;

    #[repr(C)]
    struct ExtInfo {
        flags: u64,
        reserved_addr: *const c_void,
        reserved_size: libc::size_t,
        relro_fd: libc::c_int,
        library_fd: libc::c_int,
        library_fd_offset: libc::off64_t,
        library_namespace: *const c_void,
    }

    extern "C" {
        fn android_dlopen_ext(
            filename: *const libc::c_char,
            flags: libc::c_int,
            ext_info: *const ExtInfo,
        ) -> *const c_void;
    }

    use memmap2::MmapAsRawDesc;
    use std::os::unix::prelude::{FromRawFd, IntoRawFd};

    let contents = std::fs::read(file)
        .map_err(|e| PatchError::AndroidMemfd(format!("Failed to read file: {}", e)))?;
    let mut mfd = memfd::MemfdOptions::default()
        .create("subsecond-patch")
        .map_err(|e| PatchError::AndroidMemfd(format!("Failed to create memfd: {}", e)))?;
    mfd.as_file()
        .set_len(contents.len() as u64)
        .map_err(|e| PatchError::AndroidMemfd(format!("Failed to set memfd length: {}", e)))?;

    let raw_fd = mfd.into_raw_fd();

    let mut map = memmap2::MmapMut::map_mut(raw_fd)
        .map_err(|e| PatchError::AndroidMemfd(format!("Failed to map memfd: {}", e)))?;
    map.copy_from_slice(&contents);
    let _map = map
        .make_exec()
        .map_err(|e| PatchError::AndroidMemfd(format!("Failed to make memfd executable: {}", e)))?;

    let filename = c"/subsecond-patch";

    let info = ExtInfo {
        flags: 0x10,
        reserved_addr: ptr::null(),
        reserved_size: 0,
        relro_fd: 0,
        library_fd: raw_fd,
        library_fd_offset: 0,
        library_namespace: ptr::null(),
    };

    let flags = libloading::os::unix::RTLD_LAZY | libloading::os::unix::RTLD_LOCAL;

    let handle = libloading::os::unix::with_dlerror(
        || {
            let ptr = android_dlopen_ext(filename.as_ptr() as _, flags, &info);
            if ptr.is_null() {
                None
            } else {
                Some(ptr)
            }
        },
        |err| err.to_str().unwrap_or_default().to_string(),
    )
    .map_err(|e| {
        PatchError::AndroidMemfd(format!(
            "android_dlopen_ext failed: {}",
            e.unwrap_or_default()
        ))
    })?;

    let lib = unsafe { libloading::os::unix::Library::from_raw(handle as *mut c_void) };
    let lib: libloading::Library = lib.into();
    Ok(lib)
}

pub trait HotFunction<Args, Marker> {
    type Return;
    type Real;

    fn call_it(&mut self, args: Args) -> Self::Return;
    unsafe fn call_as_ptr(&mut self, _args: Args) -> Self::Return;
}

macro_rules! impl_hot_function {
    (
        $(
            ($marker:ident, $($arg:ident),*)
        ),*
    ) => {
        $(
            #[doc(hidden)]
            pub struct $marker;

            impl<T, $($arg,)* R> HotFunction<($($arg,)*), $marker> for T
            where
                T: FnMut($($arg),*) -> R,
            {
                type Return = R;
                type Real = fn($($arg),*) -> R;

                fn call_it(&mut self, args: ($($arg,)*)) -> Self::Return {
                    #[allow(non_snake_case)]
                    let ( $($arg,)* ) = args;
                    self($($arg),*)
                }

                unsafe fn call_as_ptr(&mut self, args: ($($arg,)*)) -> Self::Return {
                    unsafe {
                        if let Some(jump_table) = get_jump_table() {
                            let real = std::mem::transmute_copy::<Self, Self::Real>(&self) as *const ();

                            #[cfg(all(target_pointer_width = "64", target_os = "android"))] let nibble  = real as u64 & 0xFF00_0000_0000_0000;
                            #[cfg(all(target_pointer_width = "64", target_os = "android"))] let real    = real as u64 & 0x00FFF_FFF_FFFF_FFFF;

                            #[cfg(target_pointer_width = "64")] let real  = real as u64;
                            #[cfg(target_pointer_width = "32")] let real = real as u64;

                            if let Some(ptr) = jump_table.map.get(&real).cloned() {
                                #[cfg(all(target_pointer_width = "64", target_os = "android"))] let ptr: u64 = ptr | nibble;
                                #[cfg(target_pointer_width = "64")] let ptr: u64 = ptr;
                                #[cfg(target_pointer_width = "32")] let ptr: u32 = ptr as u32;

                                #[allow(non_snake_case)]
                                let ( $($arg,)* ) = args;

                                #[cfg(target_pointer_width = "64")]
                                type PtrWidth = u64;
                                #[cfg(target_pointer_width = "32")]
                                type PtrWidth = u32;

                                return std::mem::transmute::<PtrWidth, Self::Real>(ptr)($($arg),*);
                            }
                        }

                        self.call_it(args)
                    }
                }
            }
        )*
    };
}

impl_hot_function!(
    (Fn0Marker,),
    (Fn1Marker, A),
    (Fn2Marker, A, B),
    (Fn3Marker, A, B, C),
    (Fn4Marker, A, B, C, D),
    (Fn5Marker, A, B, C, D, E),
    (Fn6Marker, A, B, C, D, E, F),
    (Fn7Marker, A, B, C, D, E, F, G),
    (Fn8Marker, A, B, C, D, E, F, G, H),
    (Fn9Marker, A, B, C, D, E, F, G, H, I)
);
