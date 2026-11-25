//! Thin wrappers over the Win32 calls the app needs: processes, services,
//! the registry, Task Scheduler, shortcuts, system counters and elevation.
//! Everything here is direct API use, polled by the collector about once a
//! second. No WMI, no PowerShell.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, FILETIME, HANDLE, VARIANT_BOOL};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile,
    STGM_READ,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, KEY_WRITE,
    REG_BINARY, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
    RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Services::{
    CloseServiceHandle, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    QUERY_SERVICE_CONFIGW, QueryServiceConfig2W, QueryServiceConfigW, SC_ENUM_PROCESS_INFO, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_AUTO_START, SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
    SERVICE_CONFIG_DESCRIPTION, SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DESCRIPTIONW, SERVICE_QUERY_CONFIG,
    SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_WIN32,
};
use windows::Win32::System::SystemInformation::{GetSystemTimeAsFileTime, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_ENUM_HIDDEN, TASK_RUNLEVEL_HIGHEST,
    TASK_RUNLEVEL_TYPE, TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2, TaskScheduler,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetExitCodeProcess,
    HIGH_PRIORITY_CLASS, INFINITE, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, SetPriorityClass, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_I4};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartup, FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
use windows::core::{BSTR, Interface, PCWSTR, PWSTR};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

unsafe fn from_pwstr(p: PWSTR) -> String {
    if p.is_null() { String::new() } else { unsafe { p.to_string().unwrap_or_default() } }
}

/// A FILETIME as one number of 100 ns ticks since 1601.
pub fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

/// Now, in FILETIME ticks.
pub fn now_ft() -> u64 {
    unsafe { ft(GetSystemTimeAsFileTime()) }
}

/// FILETIME ticks to Unix seconds.
pub fn ft_to_unix(t: u64) -> u64 {
    t.saturating_sub(116_444_736_000_000_000) / 10_000_000
}

/// Local date and time of a Unix time: (year, month, day, weekday 0 =
/// Sunday, hour, minute).
pub fn local_time(unix: u64) -> Option<(u16, u16, u16, u16, u16, u16)> {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let t = unix * 10_000_000 + 116_444_736_000_000_000;
    let f = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&f, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some((local.wYear, local.wMonth, local.wDay, local.wDayOfWeek, local.wHour, local.wMinute))
}

// ------------------------------------------------------------- processes

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    /// Lower-case file name, such as `teams.exe`.
    pub name: String,
    /// Lower-case full path, when Windows lets us read it.
    pub path: Option<String>,
    pub session: u32,
    /// Creation time, FILETIME ticks.
    pub created: u64,
    /// Kernel plus user time, 100 ns ticks.
    pub cpu: u64,
    /// Bytes read and written, all I/O.
    pub io: u64,
}

/// Lists this sign-in session's processes for the collector. One
/// `NtQuerySystemInformation` call returns them with their times and I/O,
/// so a tick costs one system call rather than opening each of a few
/// hundred processes; asking for this session only (services run in
/// session 0 and are read from the Service Control Manager) halves even
/// that. Only the full
/// path needs a handle, and a process's path never changes, so that is
/// looked up once, when the process is first seen.
#[derive(Default)]
pub struct ProcessLister {
    paths: HashMap<(u32, u64), Option<String>>,
    buf: Vec<u64>,
    session: Option<u32>,
}

/// The query for one session's processes (SystemSessionProcessInformation).
#[repr(C)]
struct SessionQuery {
    session: u32,
    size: u32,
    buffer: *mut c_void,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, returned: *mut u32) -> i32;
}

/// SYSTEM_PROCESS_INFORMATION as Windows lays it out (64-bit), up to the
/// I/O counters. Thread entries follow; `next` skips them.
#[repr(C)]
struct SysProc {
    next: u32,
    threads: u32,
    _private_ws: i64,
    _hard_faults: u32,
    _threads_high: u32,
    _cycles: u64,
    created: i64,
    user: i64,
    kernel: i64,
    name_len: u16,
    _name_max: u16,
    name: *const u16,
    _priority: i32,
    pid: usize,
    _parent: usize,
    _handles: u32,
    session: u32,
    _key: usize,
    _peak_virtual: usize,
    _virtual: usize,
    _page_faults: u32,
    _peak_ws: usize,
    _ws: usize,
    _quota: [usize; 4],
    _pagefile: usize,
    _peak_pagefile: usize,
    _private_pages: usize,
    _read_ops: i64,
    _write_ops: i64,
    _other_ops: i64,
    read_bytes: i64,
    write_bytes: i64,
    _other_bytes: i64,
}

const SYSTEM_SESSION_PROCESS_INFORMATION: u32 = 53;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

impl ProcessLister {
    pub fn list(&mut self) -> Vec<Proc> {
        let mut out = Vec::new();
        if self.buf.is_empty() {
            self.buf = vec![0u64; 256 * 1024 / 8];
        }
        let session = *self.session.get_or_insert_with(current_session);
        unsafe {
            loop {
                let mut needed = 0u32;
                let len = (self.buf.len() * 8) as u32;
                let mut query = SessionQuery { session, size: len, buffer: self.buf.as_mut_ptr().cast() };
                let status = NtQuerySystemInformation(
                    SYSTEM_SESSION_PROCESS_INFORMATION,
                    (&mut query as *mut SessionQuery).cast(),
                    size_of::<SessionQuery>() as u32,
                    &mut needed,
                );
                if status == STATUS_INFO_LENGTH_MISMATCH {
                    // Room for processes started since, too.
                    let want = (needed as usize).max(len as usize) * 3 / 2;
                    self.buf = vec![0u64; want.div_ceil(8)];
                    continue;
                }
                if status < 0 {
                    return out;
                }
                break;
            }
            let base = self.buf.as_ptr() as *const u8;
            let mut offset = 0usize;
            loop {
                let e = &*(base.add(offset) as *const SysProc);
                let pid = e.pid as u32;
                if pid > 4 {
                    let name = if e.name.is_null() {
                        String::new()
                    } else {
                        String::from_utf16_lossy(std::slice::from_raw_parts(e.name, e.name_len as usize / 2)).to_lowercase()
                    };
                    let created = e.created as u64;
                    let path = self.paths.entry((pid, created)).or_insert_with(|| image_path(pid)).clone();
                    out.push(Proc {
                        pid,
                        name,
                        path,
                        session: e.session,
                        created,
                        cpu: (e.user + e.kernel) as u64,
                        io: (e.read_bytes + e.write_bytes) as u64,
                    });
                }
                if e.next == 0 {
                    break;
                }
                offset += e.next as usize;
            }
        }
        let seen: std::collections::HashSet<(u32, u64)> = out.iter().map(|p| (p.pid, p.created)).collect();
        self.paths.retain(|k, _| seen.contains(k));
        out
    }
}

/// When a process started, FILETIME ticks, if Windows lets us ask: for a
/// service's host, which runs outside the session the list covers.
pub fn process_created(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut x, mut k, mut u) = Default::default();
        let ok = windows::Win32::System::Threading::GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| ft(c))
    }
}

/// A process's full program path, lower case, if Windows lets us read it.
fn image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let path = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .ok()
            .map(|_| String::from_utf16_lossy(&buf[..len as usize]).to_lowercase());
        let _ = CloseHandle(h);
        path
    }
}

pub fn current_session() -> u32 {
    let mut s = 0;
    unsafe {
        let _ = ProcessIdToSessionId(std::process::id(), &mut s);
    }
    s
}

/// Ends a process. False when Windows refuses (it may run as administrator).
pub fn end_process(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let ok = TerminateProcess(h, 1).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

// NtSuspendProcess and NtResumeProcess are not in the Win32 bindings but
// have been in ntdll, unchanged, since Windows XP. Each suspend adds one to
// every thread's suspend count, so every suspend is paired with one resume.
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

/// The real window behind the "Not responding" copy Windows shows for a
/// window that has stopped answering. user32 exports it by name but its
/// import library does not list it, so it is looked up at run time.
fn hung_window_from_ghost(ghost: windows::Win32::Foundation::HWND) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type F = unsafe extern "system" fn(HWND) -> HWND;
    unsafe {
        let user32 = GetModuleHandleW(windows::core::w!("user32.dll")).ok()?;
        let f = GetProcAddress(user32, windows::core::s!("HungWindowFromGhostWindow"))?;
        let f: F = std::mem::transmute(f);
        let real = f(ghost);
        (!real.is_invalid()).then_some(real)
    }
}

const PROCESS_SUSPEND_RESUME: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS =
    windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS(0x0800);

fn suspend_or_resume(pid: u32, suspend: bool) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_SUSPEND_RESUME, false, pid) else { return false };
        let status = if suspend { NtSuspendProcess(h) } else { NtResumeProcess(h) };
        let _ = CloseHandle(h);
        status >= 0
    }
}

/// Freezes every thread of a process. False when Windows will not let us,
/// such as for a program running as administrator.
pub fn suspend_process(pid: u32) -> bool {
    suspend_or_resume(pid, true)
}

pub fn resume_process(pid: u32) -> bool {
    suspend_or_resume(pid, false)
}

/// The process whose window is in front, seeing through the "Not
/// responding" copy of a frozen window to the process it stands for.
pub fn foreground_pid() -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let mut w = GetForegroundWindow();
        if w.is_invalid() {
            return None;
        }
        if let Some(real) = hung_window_from_ghost(w) {
            w = real;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(w, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

pub fn raise_own_priority() {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }
}

/// Does some process hold this named mutex?
pub fn mutex_exists(name: &str) -> bool {
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let w = wide(name);
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(w.as_ptr())) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

/// Holds a named mutex for the life of the process, so only one collector
/// runs per session. `None` when another process already holds it.
pub fn single_instance(name: &str) -> Option<HANDLE> {
    let w = wide(name);
    unsafe {
        let h = CreateMutexW(None, true, PCWSTR(w.as_ptr())).ok()?;
        if windows::Win32::Foundation::GetLastError() == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(h)
    }
}

// --------------------------------------------------------------- services

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub display: String,
    pub delayed: bool,
    pub description: String,
    pub binary: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
}

pub struct ServiceStatus {
    pub name: String,
    pub state: ServiceState,
    pub pid: u32,
}

struct Scm(SC_HANDLE);
impl Drop for Scm {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_scm() -> Option<Scm> {
    unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)
            .ok()
            .map(Scm)
    }
}

/// Current state of every Win32 service.
pub fn service_statuses() -> Vec<ServiceStatus> {
    let mut out = Vec::new();
    let Some(scm) = open_scm() else { return out };
    unsafe {
        let mut needed = 0u32;
        let mut count = 0u32;
        let mut resume = 0u32;
        let _ = EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut needed,
            &mut count,
            Some(&mut resume),
            PCWSTR::null(),
        );
        // u64 storage keeps the structs inside correctly aligned.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8) + 64];
        let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8);
        resume = 0;
        if EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            Some(bytes),
            &mut needed,
            &mut count,
            Some(&mut resume),
            PCWSTR::null(),
        )
        .is_err()
        {
            return out;
        }
        let items = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
        for s in items {
            let st = s.ServiceStatusProcess.dwCurrentState;
            let state = if st == SERVICE_RUNNING {
                ServiceState::Running
            } else if st == SERVICE_START_PENDING {
                ServiceState::Starting
            } else {
                ServiceState::Stopped
            };
            out.push(ServiceStatus {
                name: from_pwstr(s.lpServiceName).to_lowercase(),
                state,
                pid: s.ServiceStatusProcess.dwProcessId,
            });
        }
    }
    out
}

/// Configuration of every automatic and delayed-automatic service.
pub fn auto_services() -> Vec<ServiceConfig> {
    let mut out = Vec::new();
    let Some(scm) = open_scm() else { return out };
    let names: Vec<String> = service_statuses().into_iter().map(|s| s.name).collect();
    for name in names {
        let w = wide(&name);
        unsafe {
            let Ok(h) = OpenServiceW(scm.0, PCWSTR(w.as_ptr()), SERVICE_QUERY_CONFIG) else { continue };
            let svc = Scm(h);
            let mut needed = 0u32;
            let _ = QueryServiceConfigW(svc.0, None, 0, &mut needed);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8) + 1];
            let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            if QueryServiceConfigW(svc.0, Some(cfg), needed, &mut needed).is_err() {
                continue;
            }
            let cfg = &*cfg;
            if cfg.dwStartType != SERVICE_AUTO_START {
                continue;
            }
            let display = from_pwstr(cfg.lpDisplayName);
            let binary = from_pwstr(cfg.lpBinaryPathName);

            let mut delayed = false;
            let mut info = [0u64; 2];
            let bytes = std::slice::from_raw_parts_mut(info.as_mut_ptr() as *mut u8, 16);
            if QueryServiceConfig2W(svc.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, Some(bytes), &mut needed).is_ok() {
                let d = &*(info.as_ptr() as *const SERVICE_DELAYED_AUTO_START_INFO);
                delayed = d.fDelayedAutostart.as_bool();
            }

            let mut description = String::new();
            let _ = QueryServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, None, &mut needed);
            if needed > 0 {
                let mut buf = vec![0u64; (needed as usize).div_ceil(8) + 1];
                let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8);
                if QueryServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, Some(bytes), &mut needed).is_ok() {
                    let d = &*(buf.as_ptr() as *const SERVICE_DESCRIPTIONW);
                    description = from_pwstr(d.lpDescription);
                }
            }
            out.push(ServiceConfig { name: name.clone(), display, delayed, description, binary });
        }
    }
    out
}

// --------------------------------------------------------------- registry

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    User,
    Machine,
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn root(h: Hive) -> HKEY {
    match h {
        Hive::User => HKEY_CURRENT_USER,
        Hive::Machine => HKEY_LOCAL_MACHINE,
    }
}

fn open_key(h: Hive, path: &str, wow32: bool, write: bool) -> Option<Key> {
    let w = wide(path);
    let view = if wow32 { KEY_WOW64_32KEY } else { KEY_WOW64_64KEY };
    let access: REG_SAM_FLAGS = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    let mut key = HKEY::default();
    unsafe {
        if write {
            let r = RegCreateKeyExW(
                root(h),
                PCWSTR(w.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                access | view,
                None,
                &mut key,
                None,
            );
            (r == ERROR_SUCCESS).then_some(Key(key))
        } else {
            let r = RegOpenKeyExW(root(h), PCWSTR(w.as_ptr()), None, access | view, &mut key);
            (r == ERROR_SUCCESS).then_some(Key(key))
        }
    }
}

pub enum RegData {
    Text(String),
    Binary(Vec<u8>),
}

/// All values of a key, as (name, data). Strings are returned raw
/// (environment variables not expanded).
pub fn reg_values(h: Hive, path: &str, wow32: bool) -> Vec<(String, RegData)> {
    let mut out = Vec::new();
    let Some(key) = open_key(h, path, wow32, false) else { return out };
    let mut i = 0;
    loop {
        let mut name = [0u16; 512];
        let mut name_len = name.len() as u32;
        let mut kind = 0u32;
        let mut data = vec![0u8; 8192];
        let mut data_len = data.len() as u32;
        let r = unsafe {
            RegEnumValueW(
                key.0,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        i += 1;
        if r != ERROR_SUCCESS {
            if i > 1000 || r.0 == 259 {
                break; // ERROR_NO_MORE_ITEMS
            }
            continue;
        }
        let name = String::from_utf16_lossy(&name[..name_len as usize]);
        data.truncate(data_len as usize);
        let value = if kind == REG_BINARY.0 {
            RegData::Binary(data)
        } else {
            let words: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            RegData::Text(from_wide(&words))
        };
        out.push((name, value));
    }
    out
}

pub fn reg_get_text(h: Hive, path: &str, name: &str) -> Option<String> {
    let key = open_key(h, path, false, false)?;
    let w = wide(name);
    let mut buf = vec![0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    let mut kind = REG_VALUE_TYPE::default();
    let r = unsafe {
        RegQueryValueExW(key.0, PCWSTR(w.as_ptr()), None, Some(&mut kind), Some(buf.as_mut_ptr() as *mut u8), Some(&mut len))
    };
    (r == ERROR_SUCCESS).then(|| from_wide(&buf[..(len as usize / 2).min(buf.len())]))
}

pub fn reg_set_text(h: Hive, path: &str, name: &str, value: &str) -> bool {
    let Some(key) = open_key(h, path, false, true) else { return false };
    let n = wide(name);
    let v = wide(value);
    let bytes: Vec<u8> = v.iter().flat_map(|c| c.to_le_bytes()).collect();
    unsafe { RegSetValueExW(key.0, PCWSTR(n.as_ptr()), None, REG_SZ, Some(&bytes)) == ERROR_SUCCESS }
}

pub fn reg_set_binary(h: Hive, path: &str, name: &str, value: &[u8]) -> bool {
    let Some(key) = open_key(h, path, false, true) else { return false };
    let n = wide(name);
    unsafe { RegSetValueExW(key.0, PCWSTR(n.as_ptr()), None, REG_BINARY, Some(value)) == ERROR_SUCCESS }
}

pub fn reg_delete(h: Hive, path: &str, name: &str) -> bool {
    let Some(key) = open_key(h, path, false, true) else { return false };
    let n = wide(name);
    unsafe { RegDeleteValueW(key.0, PCWSTR(n.as_ptr())) == ERROR_SUCCESS }
}

pub fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let w = wide(s);
    let mut buf = vec![0u16; 4096];
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(w.as_ptr()), Some(&mut buf)) };
    if n == 0 || n as usize > buf.len() { s.to_owned() } else { from_wide(&buf) }
}

/// The "File description" from a program's version information, such as
/// "Microsoft Edge".
pub fn file_description(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u64; (size as usize).div_ceil(8)];
        let block = data.as_mut_ptr() as *mut c_void;
        GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, block).ok()?;
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let key = wide(r"\VarFileInfo\Translation");
        let lang = if VerQueryValueW(block, PCWSTR(key.as_ptr()), &mut ptr, &mut len).as_bool() && len >= 4 {
            let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
            format!("{:04x}{:04x}", pair[0], pair[1])
        } else {
            "040904b0".to_owned()
        };
        let key = wide(&format!(r"\StringFileInfo\{lang}\FileDescription"));
        if !VerQueryValueW(block, PCWSTR(key.as_ptr()), &mut ptr, &mut len).as_bool() || len == 0 {
            return None;
        }
        let text = from_wide(std::slice::from_raw_parts(ptr as *const u16, len as usize));
        let text = text.trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
}

// ----------------------------------------------------- startup folders

pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

pub fn startup_folder(common: bool) -> Option<std::path::PathBuf> {
    let id = if common { FOLDERID_CommonStartup } else { FOLDERID_Startup };
    unsafe {
        let p = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None).ok()?;
        let s = from_pwstr(p);
        CoTaskMemFree(Some(p.0 as *const c_void));
        Some(s.into())
    }
}

/// The target and arguments of a `.lnk` shortcut. Needs `com_init`.
pub fn resolve_shortcut(path: &std::path::Path) -> Option<(String, String)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let w = wide(&path.to_string_lossy());
        file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let mut args = [0u16; 2048];
        let _ = link.GetArguments(&mut args);
        Some((from_wide(&target), from_wide(&args)))
    }
}

// --------------------------------------------------------- Task Scheduler

#[derive(Clone, Debug)]
pub struct LogonTask {
    /// Full path such as `\Vendor\Updater`.
    pub path: String,
    pub name: String,
    /// The first program the task runs.
    pub command: String,
    /// Last run, Unix seconds (0 if never).
    pub last_run: u64,
    /// Runs as a system account, or as the user with administrator rights.
    pub as_system: bool,
}

fn variant_i4(i: i32) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = VARENUM(VT_I4.0);
        inner.Anonymous.lVal = i;
    }
    v
}

/// OLE automation date (days since 1899-12-30, local time) to Unix seconds,
/// using the given local offset from UTC in seconds.
pub fn ole_date_to_unix(date: f64, utc_offset: i64) -> u64 {
    if date <= 1.0 {
        return 0;
    }
    let local = (date - 25569.0) * 86_400.0;
    (local as i64 - utc_offset).max(0) as u64
}

fn local_utc_offset() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.Bias + if r == 2 { tz.DaylightBias } else { tz.StandardBias };
    -(bias as i64) * 60
}

/// Asks Task Scheduler when particular tasks last ran, by path: a call per
/// task, where `logon_tasks` walks the whole library. Needs `com_init`, on
/// the thread that uses it.
pub struct TaskRuns {
    root: Option<ITaskFolder>,
    offset: i64,
}

impl TaskRuns {
    pub fn open() -> Self {
        let root = unsafe {
            CoCreateInstance::<_, ITaskService>(&TaskScheduler, None, CLSCTX_INPROC_SERVER).ok().and_then(|service| {
                let empty = VARIANT::default();
                service.Connect(&empty, &empty, &empty, &empty).ok()?;
                service.GetFolder(&BSTR::from("\\")).ok()
            })
        };
        Self { root, offset: local_utc_offset() }
    }

    /// The tasks at `paths` with when each last ran (Unix seconds, 0 if
    /// never). Only `path` and `last_run` are filled in.
    pub fn last_runs(&self, paths: &[&str]) -> Vec<LogonTask> {
        let Some(root) = &self.root else { return Vec::new() };
        paths
            .iter()
            .filter_map(|path| {
                let task = unsafe { root.GetTask(&BSTR::from(*path)).ok()? };
                let last_run = unsafe { task.LastRunTime().map(|d| ole_date_to_unix(d, self.offset)).unwrap_or(0) };
                Some(LogonTask { path: (*path).to_owned(), name: String::new(), command: String::new(), last_run, as_system: false })
            })
            .collect()
    }
}

/// Enabled tasks with a logon trigger that run a program. Tasks under
/// `\Microsoft\Windows` are Windows' own housekeeping and are left out.
/// Needs `com_init`.
pub fn logon_tasks() -> Vec<LogonTask> {
    let mut out = Vec::new();
    unsafe {
        let Ok(service) = CoCreateInstance::<_, ITaskService>(&TaskScheduler, None, CLSCTX_INPROC_SERVER) else {
            return out;
        };
        let empty = VARIANT::default();
        if service.Connect(&empty, &empty, &empty, &empty).is_err() {
            return out;
        }
        let Ok(root) = service.GetFolder(&BSTR::from("\\")) else { return out };
        let offset = local_utc_offset();
        let me = Me::current();
        walk_folder(&root, &mut out, offset, 0, &me, user_is_admin());
    }
    out
}

/// Who is signed in, in the forms a task's principal may name them.
pub struct Me {
    /// `freddie`
    pub name: String,
    /// `PC\freddie`
    pub full: String,
    /// `S-1-5-21-...`
    pub sid: String,
}

impl Me {
    pub fn current() -> Self {
        let name = std::env::var("USERNAME").unwrap_or_default();
        let full = match std::env::var("USERDOMAIN") {
            Ok(d) if !d.is_empty() => format!("{d}\\{name}"),
            _ => name.clone(),
        };
        Self { name, full, sid: current_user_sid().unwrap_or_default() }
    }
}

/// Is the signed-in user an administrator? With UAC on, an administrator's
/// sign-in holds a split token (limited or full); anyone else's does not.
pub fn user_is_admin() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_QUERY, TokenElevation, TokenElevationType,
        TokenElevationTypeDefault,
    };
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut kind = TOKEN_ELEVATION_TYPE::default();
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0;
        let split = GetTokenInformation(
            token,
            TokenElevationType,
            Some((&mut kind as *mut TOKEN_ELEVATION_TYPE).cast()),
            size_of::<TOKEN_ELEVATION_TYPE>() as u32,
            &mut len,
        )
        .is_ok()
            && kind != TokenElevationTypeDefault;
        let elevated = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok()
            && elevation.TokenIsElevated != 0;
        let _ = CloseHandle(token);
        split || elevated
    }
}

/// This process's user as an `S-1-...` string.
fn current_user_sid() -> Option<String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut buf = vec![0u8; 512];
        let mut len = 0;
        let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), buf.len() as u32, &mut len);
        let _ = CloseHandle(token);
        got.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid
    }
}

/// Does a task whose principal is `user_id` run when `me` signs in? Tasks
/// for the system accounts and for groups do; a task that names another
/// person (their own OneDrive task, say) never runs in this session.
pub fn task_is_for(user_id: &str, me: &Me) -> bool {
    let u = user_id.trim().to_lowercase();
    if u.is_empty() || u.starts_with("builtin\\") || u.starts_with("s-1-5-32-") {
        return true;
    }
    if is_system_account(&u) {
        return true;
    }
    let name = me.name.to_lowercase();
    if u.starts_with("s-1-") {
        return !me.sid.is_empty() && u == me.sid.to_lowercase();
    }
    u == me.full.to_lowercase() || (!name.is_empty() && u.rsplit('\\').next() == Some(name.as_str()))
}

/// SYSTEM, LOCAL SERVICE or NETWORK SERVICE, in any of the ways a task's
/// principal names them.
pub fn is_system_account(user_id: &str) -> bool {
    const SYSTEM: [&str; 9] = [
        "system",
        "nt authority\\system",
        "s-1-5-18",
        "local service",
        "nt authority\\local service",
        "s-1-5-19",
        "network service",
        "nt authority\\network service",
        "s-1-5-20",
    ];
    SYSTEM.contains(&user_id.trim().to_lowercase().as_str())
}

unsafe fn walk_folder(folder: &ITaskFolder, out: &mut Vec<LogonTask>, offset: i64, depth: u32, me: &Me, admin: bool) {
    unsafe {
        let path = folder.Path().map(|b| b.to_string()).unwrap_or_default();
        if path.to_lowercase().starts_with("\\microsoft\\windows") || depth > 6 {
            return;
        }
        if let Ok(tasks) = folder.GetTasks(TASK_ENUM_HIDDEN.0) {
            let n = tasks.Count().unwrap_or(0);
            for i in 1..=n {
                let Ok(task) = tasks.get_Item(&variant_i4(i)) else { continue };
                if task.Enabled().map(|b: VARIANT_BOOL| !b.as_bool()).unwrap_or(true) {
                    continue;
                }
                let Ok(def) = task.Definition() else { continue };
                let mut owner = BSTR::new();
                let mut level = TASK_RUNLEVEL_TYPE::default();
                if let Ok(principal) = def.Principal() {
                    let _ = principal.UserId(&mut owner);
                    let _ = principal.RunLevel(&mut level);
                }
                // "Highest" means administrator only for an administrator's
                // account; for anyone else it runs as them like any app.
                let as_system = is_system_account(&owner.to_string()) || (admin && level == TASK_RUNLEVEL_HIGHEST);
                if !task_is_for(&owner.to_string(), me) {
                    continue;
                }
                let mut logon = false;
                if let Ok(triggers) = def.Triggers() {
                    let mut count = 0;
                    let _ = triggers.Count(&mut count);
                    for t in 1..=count {
                        if let Ok(trigger) = triggers.get_Item(t) {
                            let mut kind = TASK_TRIGGER_TYPE2::default();
                            let mut enabled = VARIANT_BOOL::default();
                            let _ = trigger.Type(&mut kind);
                            let _ = trigger.Enabled(&mut enabled);
                            if kind == TASK_TRIGGER_LOGON && enabled.as_bool() {
                                logon = true;
                            }
                        }
                    }
                }
                if !logon {
                    continue;
                }
                let mut command = String::new();
                if let Ok(actions) = def.Actions() {
                    let mut count = 0;
                    let _ = actions.Count(&mut count);
                    for a in 1..=count {
                        let Ok(action) = actions.get_Item(a) else { continue };
                        let mut kind = Default::default();
                        let _ = action.Type(&mut kind);
                        if kind != TASK_ACTION_EXEC {
                            continue;
                        }
                        if let Ok(exec) = action.cast::<IExecAction>() {
                            let mut p = BSTR::new();
                            let mut args = BSTR::new();
                            let _ = exec.Path(&mut p);
                            let _ = exec.Arguments(&mut args);
                            let program = p.to_string();
                            let program = program.trim().trim_matches('"');
                            if program.is_empty() {
                                continue;
                            }
                            command = format!("\"{program}\" {args}").trim().to_owned();
                            break;
                        }
                    }
                }
                if command.is_empty() {
                    continue;
                }
                out.push(LogonTask {
                    path: task.Path().map(|b| b.to_string()).unwrap_or_default(),
                    name: task.Name().map(|b| b.to_string()).unwrap_or_default(),
                    command,
                    last_run: task.LastRunTime().map(|d| ole_date_to_unix(d, offset)).unwrap_or(0),
                    as_system,
                });
            }
        }
        if let Ok(subs) = folder.GetFolders(0) {
            let n = subs.Count().unwrap_or(0);
            for i in 1..=n {
                if let Ok(sub) = subs.get_Item(&variant_i4(i)) {
                    walk_folder(&sub, out, offset, depth + 1, me, admin);
                }
            }
        }
    }
}

// --------------------------------------------------------- system counters

/// Whole-machine CPU (percent) and disk (bytes per second). CPU comes from
/// `GetSystemTimes` and disk from each physical disk's own byte counters,
/// both a call or two a second. (Windows' performance-counter library did
/// the same but took over a third of a second of CPU just to open.)
pub struct Counters {
    times: (u64, u64),
    disks: Vec<HANDLE>,
    bytes: u64,
    at: std::time::Instant,
}

// The disk handles are owned by this struct.
unsafe impl Send for Counters {}

impl Drop for Counters {
    fn drop(&mut self) {
        for h in self.disks.drain(..) {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

/// (busy, total) processor time so far, 100 ns ticks over all processors.
fn system_times() -> (u64, u64) {
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
            return (0, 0);
        }
    }
    // Kernel time includes idle time.
    let total = ft(kernel) + ft(user);
    (total.saturating_sub(ft(idle)), total)
}

/// Bytes read and written so far by one physical disk.
fn disk_bytes(h: HANDLE) -> Option<u64> {
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE};
    let mut perf = DISK_PERFORMANCE::default();
    let mut len = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_PERFORMANCE,
            None,
            0,
            Some(&mut perf as *mut DISK_PERFORMANCE as *mut c_void),
            size_of::<DISK_PERFORMANCE>() as u32,
            Some(&mut len),
            None,
        )
        .ok()?;
    }
    Some(perf.BytesRead as u64 + perf.BytesWritten as u64)
}

impl Counters {
    pub fn open() -> Option<Self> {
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        // No access rights are needed to ask a disk for its counters.
        let mut disks = Vec::new();
        for n in 0..16 {
            let name = wide(&format!(r"\\.\PhysicalDrive{n}"));
            let opened = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                )
            };
            match opened {
                Ok(h) if disk_bytes(h).is_some() => disks.push(h),
                Ok(h) => unsafe {
                    let _ = CloseHandle(h);
                },
                Err(_) => {}
            }
        }
        let bytes = disks.iter().filter_map(|&h| disk_bytes(h)).sum();
        Some(Self { times: system_times(), disks, bytes, at: std::time::Instant::now() })
    }

    /// (cpu percent, disk bytes/s) since the last call.
    pub fn sample(&mut self) -> (f32, f64) {
        let times = system_times();
        let busy = times.0.saturating_sub(self.times.0) as f64;
        let total = times.1.saturating_sub(self.times.1) as f64;
        self.times = times;
        let cpu = if total > 0.0 { (busy / total * 100.0).clamp(0.0, 100.0) as f32 } else { 0.0 };
        let bytes: u64 = self.disks.iter().filter_map(|&h| disk_bytes(h)).sum();
        let secs = self.at.elapsed().as_secs_f64().max(0.001);
        let disk = bytes.saturating_sub(self.bytes) as f64 / secs;
        self.bytes = bytes;
        self.at = std::time::Instant::now();
        (cpu, disk)
    }
}

/// Memory in use, percent.
pub fn memory_load() -> f32 {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        if GlobalMemoryStatusEx(&mut m).is_ok() { m.dwMemoryLoad as f32 } else { 0.0 }
    }
}

/// Gives back the memory pages this process is not using right now. Once
/// startup work (reading the startup list, history and so on) is over, the
/// collector needs only a few hundred kilobytes a second; the rest of what
/// it touched can go back to Windows.
pub fn trim_memory() {
    unsafe {
        let _ = windows::Win32::System::ProcessStatus::EmptyWorkingSet(GetCurrentProcess());
    }
}

/// The system DPI as a scale (1.0 at 96 DPI).
pub fn system_scale() -> f32 {
    unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem().max(96) as f32 / 96.0 }
}

/// Does the user have apps set to the dark look?
pub fn apps_use_dark() -> bool {
    let Some(key) = open_key(Hive::User, r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", false, false) else {
        return false;
    };
    let name = wide("AppsUseLightTheme");
    let mut value = 1u32;
    let mut len = 4u32;
    unsafe {
        let _ = RegQueryValueExW(key.0, PCWSTR(name.as_ptr()), None, None, Some(&mut value as *mut u32 as *mut u8), Some(&mut len));
    }
    value == 0
}

/// Lets this process draw sharply on every screen. The collector calls it
/// before making the overlay; eframe does the same for the full window.
pub fn dpi_aware() {
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Brings this app's full window to the front if another process has it
/// open. True if there was one.
pub fn focus_existing_window() -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextW, IsIconic, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> windows::core::BOOL {
        unsafe {
            let mut title = [0u16; 128];
            let n = GetWindowTextW(window, &mut title) as usize;
            if !IsWindowVisible(window).as_bool() || String::from_utf16_lossy(&title[..n]) != crate::brand::NAME {
                return true.into();
            }
            let mut class = [0u16; 128];
            let n = GetClassNameW(window, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            // Not the tray's hidden window or the overlay: the full window.
            if class == "OverlayWindow" || crate::brand::TRAY_CLASS.to_string().is_ok_and(|t| t == class) {
                return true.into();
            }
            *(found.0 as *mut isize) = window.0 as isize;
            false.into()
        }
    }
    let mut found: isize = 0;
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut isize as isize));
        if found == 0 {
            return false;
        }
        let window = HWND(found as *mut _);
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        let _ = SetForegroundWindow(window);
    }
    true
}

/// The desktop area not covered by the taskbar, in points: (left, top,
/// right, bottom).
pub fn work_area_points() -> Option<(f32, f32, f32, f32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut r = RECT::default();
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut r as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
        let scale = GetDpiForSystem().max(96) as f32 / 96.0;
        Some((r.left as f32 / scale, r.top as f32 / scale, r.right as f32 / scale, r.bottom as f32 / scale))
    }
}

// --------------------------------------------------- launching programs

/// Runs a program and, for `wait`, waits for it and returns its exit code.
/// `elevate` asks for administrator approval first.
pub fn shell_run(file: &str, params: &str, elevate: bool, wait: bool, hidden: bool) -> Option<u32> {
    let verb = wide(if elevate { "runas" } else { "open" });
    let f = wide(file);
    let p = wide(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(f.as_ptr()),
        lpParameters: if params.is_empty() { PCWSTR::null() } else { PCWSTR(p.as_ptr()) },
        nShow: if hidden { SW_HIDE.0 } else { SW_SHOWNORMAL.0 },
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).ok()?;
        let h = info.hProcess;
        if h.is_invalid() {
            return Some(0);
        }
        let mut code = 0;
        if wait {
            WaitForSingleObject(h, INFINITE);
            let _ = GetExitCodeProcess(h, &mut code);
        }
        let _ = CloseHandle(h);
        Some(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ole_dates_convert_to_unix_time() {
        // 2024-01-01 00:00 UTC is OLE date 45292.
        assert_eq!(ole_date_to_unix(45292.0, 0), 1_704_067_200);
        assert_eq!(ole_date_to_unix(45292.0, 3600), 1_704_067_200 - 3600);
        assert_eq!(ole_date_to_unix(0.0, 0), 0);
    }

    #[test]
    fn filetime_converts_to_unix_time() {
        assert_eq!(ft_to_unix(116_444_736_000_000_000), 0);
        assert_eq!(ft_to_unix(116_444_736_000_000_000 + 10_000_000 * 60), 60);
    }
}

/// "Windows 11 Pro 24H2 (build 26100)", read from the registry.
pub fn windows_version() -> String {
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let get = |name: &str| reg_get_text(Hive::Machine, key, name).unwrap_or_default();
    let build = get("CurrentBuild");
    // Windows 11 still calls itself "Windows 10" in this registry value.
    let name = if build.parse::<u32>().unwrap_or(0) >= 22_000 { get("ProductName").replace("Windows 10", "Windows 11") } else { get("ProductName") };
    format!("{name} {} (build {build})", get("DisplayVersion"))
}

/// This process's working set and private bytes.
pub fn own_memory() -> (usize, usize) {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
    let mut m = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe {
        let p = &mut m as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS;
        if GetProcessMemoryInfo(GetCurrentProcess(), p, size).is_ok() {
            return (m.WorkingSetSize, m.PrivateUsage);
        }
    }
    (0, 0)
}

#[cfg(test)]
mod task_owner_tests {
    use super::*;

    #[test]
    fn only_tasks_that_run_at_my_sign_in_are_kept() {
        let me = Me { name: "Freddie".into(), full: "PC\\Freddie".into(), sid: "S-1-5-21-1-2-3-1001".into() };
        for mine in ["", "PC\\freddie", "freddie", "AzureAD\\Freddie", "S-1-5-21-1-2-3-1001", "SYSTEM", "NT AUTHORITY\\SYSTEM", "S-1-5-18", "BUILTIN\\Users", "S-1-5-32-545"] {
            assert!(task_is_for(mine, &me), "{mine}");
        }
        for theirs in ["PC\\Alex", "alex", "S-1-5-21-1-2-3-1009", "S-1-12-1-1175216000-1242900198-1137631668-4064571146"] {
            assert!(!task_is_for(theirs, &me), "{theirs}");
        }
    }
}

#[cfg(test)]
mod live_task_tests {
    /// Lists this PC's sign-in tasks and whether each runs as system: run
    /// with --ignored --nocapture.
    #[test]
    #[ignore]
    fn live_lists_sign_in_tasks() {
        super::com_init();
        for t in super::logon_tasks() {
            println!("{:<6} {}", if t.as_system { "system" } else { "yours" }, t.path);
        }
    }
}

#[cfg(test)]
mod lister_tests {
    use super::*;

    /// The one-call list agrees with asking a process directly.
    #[test]
    fn lists_this_process_like_windows_reports_it() {
        let me = std::process::id();
        let list = ProcessLister::default().list();
        let p = list.iter().find(|p| p.pid == me).expect("this process listed");
        assert_eq!(p.session, current_session());
        assert!(p.name.ends_with(".exe"), "{}", p.name);
        assert!(p.path.as_deref().is_some_and(|x| x.ends_with(&p.name)), "{:?}", p.path);
        use windows::Win32::System::Threading::GetProcessTimes;
        let (mut c, mut x, mut k, mut u) = Default::default();
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut x, &mut k, &mut u).unwrap() };
        assert_eq!(p.created, ft(c));
        assert!(p.cpu <= ft(k) + ft(u) && p.cpu + 10_000_000 > ft(k) + ft(u));
        assert!(list.len() > 20 && list.iter().any(|p| p.name == "explorer.exe" || p.name == "csrss.exe"));
    }
}

#[cfg(test)]
mod counter_tests {
    /// Disk and CPU readings move when the PC works: run with --ignored.
    #[test]
    #[ignore]
    fn live_counters_follow_real_work() {
        let mut c = super::Counters::open().unwrap();
        assert!(!c.disks.is_empty(), "no physical disk could be read");
        let path = std::env::temp_dir().join("counters-test.bin");
        let block = vec![7u8; 64 * 1024 * 1024];
        c.sample();
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&block).unwrap();
            f.sync_all().unwrap();
        }
        let mut x = 0u64;
        let t = std::time::Instant::now();
        while t.elapsed().as_millis() < 300 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        let (cpu, disk) = c.sample();
        let _ = std::fs::remove_file(&path);
        println!("cpu {cpu:.1}% disk {:.1} MB/s ({x})", disk / 1_048_576.0);
        assert!(disk > 10.0 * 1_048_576.0, "disk {disk}");
        assert!(cpu > 0.0);
    }
}

#[cfg(test)]
mod lister_cost {
    /// Splits the list's cost into Windows' call and our handling of it.
    #[test]
    #[ignore]
    fn live_lister_cost() {
        let mut l = super::ProcessLister::default();
        l.list();
        let n = 200;
        let t = std::time::Instant::now();
        for _ in 0..n {
            let mut needed = 0u32;
            let mut q = super::SessionQuery { session: super::current_session(), size: (l.buf.len() * 8) as u32, buffer: l.buf.as_mut_ptr().cast() };
            unsafe {
                super::NtQuerySystemInformation(53, (&mut q as *mut super::SessionQuery).cast(), size_of::<super::SessionQuery>() as u32, &mut needed);
            }
        }
        let call = t.elapsed().as_micros() / n;
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(l.list());
        }
        let all = t.elapsed().as_micros() / n;
        println!("system call {call} us, whole list {all} us, {} processes", l.list().len());
        let time = |name: &str, f: &mut dyn FnMut()| {
            let t = std::time::Instant::now();
            for _ in 0..50 {
                f();
            }
            println!("{name:<18} {} us", t.elapsed().as_micros() / 50);
        };
        time("service statuses", &mut || drop(std::hint::black_box(super::service_statuses())));
        time("memory load", &mut || drop(std::hint::black_box(super::memory_load())));
        let mut c = super::Counters::open().unwrap();
        time("counters", &mut || drop(std::hint::black_box(c.sample())));
        time("now", &mut || drop(std::hint::black_box(super::now_ft())));
    }
}
