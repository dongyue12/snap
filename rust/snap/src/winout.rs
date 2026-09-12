//! 输出：在 Windows 真实控制台上用 WriteConsoleW 写 UTF-16，其余情况写 UTF-8 字节。
//!
//! 这样中文在代码页 936 的 PowerShell / cmd 里也能正常显示，而重定向到文件或
//! 管道时仍然是 UTF-8（和其它命令行工具一致）。

use std::io::{self, Write};
use std::sync::OnceLock;

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(nstdhandle: u32) -> *mut c_void;
        fn GetConsoleMode(hconsolehandle: *mut c_void, lpmode: *mut u32) -> i32;
        fn WriteConsoleW(
            hconsoleoutput: *mut c_void,
            lpbuffer: *const u16,
            nnumberofcharstowrite: u32,
            lpnumberofcharswritten: *mut u32,
            lpreserved: *mut c_void,
        ) -> i32;
    }

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;

    fn handle(is_err: bool) -> *mut c_void {
        unsafe {
            GetStdHandle(if is_err {
                STD_ERROR_HANDLE
            } else {
                STD_OUTPUT_HANDLE
            })
        }
    }

    pub fn is_console(is_err: bool) -> bool {
        let h = handle(is_err);
        if h.is_null() {
            return false;
        }
        let mut mode = 0u32;
        unsafe { GetConsoleMode(h, &mut mode) != 0 }
    }

    pub fn write(is_err: bool, s: &str) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let mut written = 0u32;
        unsafe {
            WriteConsoleW(
                handle(is_err),
                wide.as_ptr(),
                wide.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            );
        }
    }
}

fn console(is_err: bool) -> bool {
    static OUT: OnceLock<bool> = OnceLock::new();
    static ERR: OnceLock<bool> = OnceLock::new();
    let cell = if is_err { &ERR } else { &OUT };
    *cell.get_or_init(|| {
        #[cfg(windows)]
        {
            win::is_console(is_err)
        }
        #[cfg(not(windows))]
        {
            // Unix: 只有是 tty 时才走同一个分支，其余写字节
            is_tty(is_err)
        }
    })
}

#[cfg(not(windows))]
fn is_tty(is_err: bool) -> bool {
    // 不引入额外依赖，用 std 判断不了 tty；直接按非控制台处理（写 UTF-8 字节即可）。
    let _ = is_err;
    false
}

/// 写一段文本到标准输出（不自动换行）。
pub fn out(s: &str) {
    write_to(false, s);
}

/// 写一段文本到标准错误（不自动换行）。
pub fn err(s: &str) {
    write_to(true, s);
}

fn write_to(is_err: bool, s: &str) {
    if console(is_err) {
        #[cfg(windows)]
        {
            win::write(is_err, s);
            return;
        }
    }
    // 重定向/管道时按平台习惯换行，与 Python 的文本模式输出保持一致
    #[cfg(windows)]
    let text = s.replace('\n', "\r\n");
    #[cfg(not(windows))]
    let text = s.to_string();

    if is_err {
        let stderr = io::stderr();
        let mut lock = stderr.lock();
        let _ = lock.write_all(text.as_bytes());
        let _ = lock.flush();
    } else {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(text.as_bytes());
        let _ = lock.flush();
    }
}

/// 进度提示：只在真正的终端上显示（管道/重定向时保持安静），用的是行内刷新。
pub fn progress(s: &str) {
    if console(true) {
        let text = format!("\r{s}    ");
        #[cfg(windows)]
        {
            win::write(true, &text);
        }
        #[cfg(not(windows))]
        {
            let _ = io::stderr().write_all(text.as_bytes());
        }
    }
}

/// 进度结束，补一个换行。
pub fn progress_done() {
    if console(true) {
        #[cfg(windows)]
        {
            win::write(true, "\n");
        }
        #[cfg(not(windows))]
        {
            let _ = io::stderr().write_all(b"\n");
        }
    }
}

/// 输出一行（stdout，自动补换行）。
#[macro_export]
macro_rules! outln {
    () => { $crate::winout::out("\n") };
    ($($arg:tt)*) => { $crate::winout::out(&format!("{}\n", format!($($arg)*))) };
}

/// 输出一行（stderr，自动补换行）。
#[macro_export]
macro_rules! errln {
    () => { $crate::winout::err("\n") };
    ($($arg:tt)*) => { $crate::winout::err(&format!("{}\n", format!($($arg)*))) };
}
