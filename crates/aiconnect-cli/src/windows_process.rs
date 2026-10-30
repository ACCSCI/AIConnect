//! Spawn without inheriting CLI capture pipes into the background daemon.
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt,
    path::Path,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT},
    System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess,
        PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
    },
};

/// A detached daemon opens its own log after startup. This preserves diagnostics
/// without inheriting any handles (including captured CLI pipes) from its parent.
pub fn redirect_output(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let handle = file.as_raw_handle() as HANDLE;
    if unsafe { SetStdHandle(STD_OUTPUT_HANDLE, handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

pub struct Child {
    handle: HANDLE,
}
impl Child {
    pub fn try_wait(&mut self) -> io::Result<Option<u32>> {
        if unsafe { WaitForSingleObject(self.handle, 0) } == WAIT_TIMEOUT {
            return Ok(None);
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.handle, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(code))
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

fn quote(argument: &OsStr) -> Vec<u16> {
    let mut output = vec![b'"' as u16];
    let mut slashes = 0;
    for unit in argument.encode_wide() {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    output
}

pub fn spawn(executable: &Path, arguments: &[OsString]) -> io::Result<Child> {
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut command = quote(executable.as_os_str());
    for argument in arguments {
        command.push(b' ' as u16);
        command.extend(quote(argument));
    }
    command.push(0);
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        CloseHandle(info.hThread);
    }
    Ok(Child {
        handle: info.hProcess,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_preserve_spaces_quotes_and_trailing_backslashes() {
        assert_eq!(
            String::from_utf16(&quote(OsStr::new("a b"))).unwrap(),
            "\"a b\""
        );
        assert_eq!(
            String::from_utf16(&quote(OsStr::new("a\"b"))).unwrap(),
            "\"a\\\"b\""
        );
        assert_eq!(
            String::from_utf16(&quote(OsStr::new("C:\\dir\\"))).unwrap(),
            "\"C:\\dir\\\\\""
        );
    }
}
