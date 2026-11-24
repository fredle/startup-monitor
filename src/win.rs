//! Thin wrappers over the Win32 calls Windows Startup Monitor needs: processes, services,
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
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCollectQueryData,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
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
    IExecAction, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_ENUM_HIDDEN,
    TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2, TaskScheduler,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetExitCodeProcess, GetProcessIoCounters, GetProcessTimes,
    HIGH_PRIORITY_CLASS, INFINITE, IO_COUNTERS, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
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

/// Lists running processes. Keeps image paths between calls so each process
/// pays for that lookup once.
#[derive(Default)]
pub struct ProcessLister {
    paths: HashMap<(u32, u64), Option<String>>,
}

impl ProcessLister {
    pub fn list(&mut self) -> Vec<Proc> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let pid = e.th32ProcessID;
                if pid > 4 {
                    let name = from_wide(&e.szExeFile).to_lowercase();
                    if let Some(p) = self.query(pid, name) {
                        seen.insert((p.pid, p.created));
                        out.push(p);
                    }
                }
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        self.paths.retain(|k, _| seen.contains(k));
        out
    }

    unsafe fn query(&mut self, pid: u32, name: String) -> Option<Proc> {
        unsafe {
            let mut session = 0;
            let _ = ProcessIdToSessionId(pid, &mut session);
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let (mut c, mut x, mut k, mut u) = Default::default();
            let times = GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u);
            let mut io = IO_COUNTERS::default();
            let _ = GetProcessIoCounters(h, &mut io);
            let created = ft(c);
            let path = self
                .paths
                .entry((pid, created))
                .or_insert_with(|| {
                    let mut buf = [0u16; 1024];
                    let mut len = buf.len() as u32;
                    QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                        .ok()
                        .map(|_| String::from_utf16_lossy(&buf[..len as usize]).to_lowercase())
                })
                .clone();
            let _ = CloseHandle(h);
            times.ok()?;
            Some(Proc {
                pid,
                name,
                path,
                session,
                created,
                cpu: ft(k) + ft(u),
                io: io.ReadTransferCount + io.WriteTransferCount,
            })
        }
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

pub fn raise_own_priority() {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
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
        walk_folder(&root, &mut out, offset, 0, &me);
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
    if SYSTEM.contains(&u.as_str()) {
        return true;
    }
    let name = me.name.to_lowercase();
    if u.starts_with("s-1-") {
        return !me.sid.is_empty() && u == me.sid.to_lowercase();
    }
    u == me.full.to_lowercase() || (!name.is_empty() && u.rsplit('\\').next() == Some(name.as_str()))
}

unsafe fn walk_folder(folder: &ITaskFolder, out: &mut Vec<LogonTask>, offset: i64, depth: u32, me: &Me) {
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
                if let Ok(principal) = def.Principal() {
                    let _ = principal.UserId(&mut owner);
                }
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
                });
            }
        }
        if let Ok(subs) = folder.GetFolders(0) {
            let n = subs.Count().unwrap_or(0);
            for i in 1..=n {
                if let Ok(sub) = subs.get_Item(&variant_i4(i)) {
                    walk_folder(&sub, out, offset, depth + 1, me);
                }
            }
        }
    }
}

// --------------------------------------------------------- system counters

/// Whole-machine CPU (percent) and disk (bytes per second) through PDH.
pub struct Counters {
    query: PDH_HQUERY,
    cpu: PDH_HCOUNTER,
    disk: PDH_HCOUNTER,
}

// PDH handles are plain handles owned by this struct.
unsafe impl Send for Counters {}

impl Counters {
    pub fn open() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut cpu = PDH_HCOUNTER::default();
            let mut disk = PDH_HCOUNTER::default();
            let c = wide("\\Processor(_Total)\\% Processor Time");
            let d = wide("\\PhysicalDisk(_Total)\\Disk Bytes/sec");
            let _ = PdhAddEnglishCounterW(query, PCWSTR(c.as_ptr()), 0, &mut cpu);
            let _ = PdhAddEnglishCounterW(query, PCWSTR(d.as_ptr()), 0, &mut disk);
            let _ = PdhCollectQueryData(query);
            Some(Self { query, cpu, disk })
        }
    }

    /// (cpu percent, disk bytes/s) since the last call.
    pub fn sample(&mut self) -> (f32, f64) {
        unsafe {
            let _ = PdhCollectQueryData(self.query);
            let read = |c: PDH_HCOUNTER| {
                let mut v = PDH_FMT_COUNTERVALUE::default();
                if PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE, None, &mut v) == 0 {
                    v.Anonymous.doubleValue
                } else {
                    0.0
                }
            };
            (read(self.cpu).clamp(0.0, 100.0) as f32, read(self.disk).max(0.0))
        }
    }
}

/// Memory in use, percent.
pub fn memory_load() -> f32 {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        if GlobalMemoryStatusEx(&mut m).is_ok() { m.dwMemoryLoad as f32 } else { 0.0 }
    }
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
