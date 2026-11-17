// Copyright (C) 2021 O.S. Systems Software LTDA
//
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Switch the calling thread to a UTF-8 locale, so libarchive hands out
/// filenames in UTF-8 whatever locale the environment sets. We restrict to
/// change LC_CTYPE only, since libarchive only needs the charset set. The
/// timing of locale setting for Unix and Windows is different, handle them
/// separately.
///
/// See on libarchive Website for a more complete description of the issue:
///
///   https://github.com/libarchive/libarchive/issues/587
///   https://github.com/libarchive/libarchive/wiki/Filenames
pub(crate) use inner::UTF8LocaleGuard;
pub(crate) use inner::WindowsUTF8LocaleGuard;

#[cfg(unix)]
mod inner {
    use std::{cell::Cell, marker::PhantomData, sync::OnceLock};

    thread_local! {
        static DEPTH: Cell<usize> = const { Cell::new(0) };
        static SAVED: Cell<libc::locale_t> = const { Cell::new(std::ptr::null_mut()) };
    }

    /// Keeps its thread in a UTF-8 locale while any guard on that thread is
    /// alive. The first guard saves the thread's locale and the last one
    /// dropped restores it, so guards may be dropped in any order: an
    /// `ArchiveIterator` holds one for its whole life. The count is per
    /// thread, so `_not_send` keeps a guard, and anything holding one, on
    /// the thread that made it.
    pub(crate) struct UTF8LocaleGuard {
        active: bool,
        _not_send: PhantomData<*mut ()>,
    }

    pub(crate) struct WindowsUTF8LocaleGuard {}

    // Made once and never freed, so that a thread whose guards are dropped
    // out of order is never left using a freed locale. NULL if none of the
    // names can be loaded.
    fn utf8_locale() -> libc::locale_t {
        static LOCALE: OnceLock<usize> = OnceLock::new();

        *LOCALE.get_or_init(|| {
            // libarchive converts a name stored in another charset (a RAR
            // UTF-16 name, a ZIP name flagged UTF-8) into the thread's
            // charset, and gives no name when it cannot. "" is C under
            // `LC_ALL=C` or with no locale set, so ask for UTF-8 by name first.
            #[cfg(any(target_os = "linux", target_os = "illumos"))]
            const LOCALES: &[&[u8]] = &[b"C.UTF-8\0", b"C.utf8\0", b"\0"];

            #[cfg(target_os = "macos")]
            const LOCALES: &[&[u8]] = &[b"UTF-8\0"];

            LOCALES
                .iter()
                .map(|locale| unsafe {
                    libc::newlocale(
                        libc::LC_CTYPE_MASK,
                        locale.as_ptr() as *const libc::c_char,
                        std::ptr::null_mut(),
                    )
                })
                .find(|locale| !locale.is_null())
                .unwrap_or(std::ptr::null_mut()) as usize
        }) as libc::locale_t
    }

    impl UTF8LocaleGuard {
        pub(crate) fn new() -> Self {
            let utf8_locale = utf8_locale();
            let active = !utf8_locale.is_null();

            if active {
                // Installed by every guard, not only the first, in case the
                // caller switched the thread's locale since then.
                let previous = unsafe { libc::uselocale(utf8_locale) };
                let depth = DEPTH.get();
                if depth == 0 {
                    SAVED.set(previous);
                }
                DEPTH.set(depth + 1);
            }

            Self {
                active,
                _not_send: PhantomData,
            }
        }
    }

    impl Drop for UTF8LocaleGuard {
        fn drop(&mut self) {
            if self.active {
                let depth = DEPTH.get() - 1;
                DEPTH.set(depth);
                if depth == 0 {
                    unsafe { libc::uselocale(SAVED.get()) };
                }
            }
        }
    }

    impl WindowsUTF8LocaleGuard {
        pub(crate) fn new() -> Self {
            Self {}
        }
    }
}

#[cfg(windows)]
mod inner {
    extern "C" {
        fn _configthreadlocale(arg1: std::os::raw::c_int) -> std::os::raw::c_int;
    }
    const _ENABLE_PER_THREAD_LOCALE: std::os::raw::c_int = 1;

    pub(crate) struct UTF8LocaleGuard {}

    pub(crate) struct WindowsUTF8LocaleGuard {
        save: Option<std::ffi::CString>,
        save_thread_config: ::std::os::raw::c_int,
    }

    impl UTF8LocaleGuard {
        pub(crate) fn new() -> Self {
            Self {}
        }
    }

    impl WindowsUTF8LocaleGuard {
        pub(crate) fn new() -> Self {
            let locale = b".UTF-8\0";

            let (save, save_thread_config) = {
                let old_locale = unsafe { libc::setlocale(libc::LC_CTYPE, std::ptr::null()) };
                (
                    if old_locale.is_null() {
                        None
                    } else {
                        Some(unsafe { std::ffi::CStr::from_ptr(old_locale) }.to_owned())
                    },
                    unsafe { _configthreadlocale(0) },
                )
            };

            unsafe {
                _configthreadlocale(_ENABLE_PER_THREAD_LOCALE);
                libc::setlocale(
                    libc::LC_CTYPE,
                    std::ffi::CStr::from_bytes_with_nul_unchecked(locale).as_ptr(),
                )
            };

            Self {
                save,
                save_thread_config,
            }
        }
    }

    impl Drop for WindowsUTF8LocaleGuard {
        fn drop(&mut self) {
            if let Some(locale) = &self.save {
                unsafe { libc::setlocale(libc::LC_CTYPE, locale.as_ptr()) };
            }

            unsafe {
                _configthreadlocale(self.save_thread_config);
            }
        }
    }
}
