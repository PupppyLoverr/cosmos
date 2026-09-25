//! Windows sandbox: AppContainer (no capabilities → no network) + Job object.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

use anyhow::{bail, Context, Result};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT,
    HLOCAL, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, TreeSetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TREE_SEC_INFO_SET, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows::Win32::Security::{
    CreateRestrictedToken, SetTokenInformation, TokenIntegrityLevel, ACL, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, DISABLE_MAX_PRIVILEGE, OBJECT_INHERIT_ACE,
    OBJECT_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_CAPABILITIES,
    SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_QUERY_SOURCE,
};
use windows::Win32::Storage::FileSystem::ReadFile;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, CreateProcessWithTokenW, DeleteProcThreadAttributeList, GetCurrentProcess,
    GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken, ResumeThread,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, EXTENDED_STARTUPINFO_PRESENT,
    LOGON_WITH_PROFILE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, STARTUPINFOW,
};

/// SE_GROUP_INTEGRITY
const SE_GROUP_INTEGRITY: u32 = 0x20;

use crate::{ExecOpts, ExecResult, NetPolicy};

const PROFILE_NAME: &str = "CosmosZSandbox";
const PROFILE_DISPLAY: &str = "Cosmos ZSandbox";
/// well-known capability: internetClient
const SID_INTERNET_CLIENT: &str = "S-1-15-3-1";
/// low integrity: S-1-16-4096
const SID_LOW_INTEGRITY: &str = "S-1-16-4096";
/// PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES = 9 | PROC_THREAD_ATTRIBUTE_INPUT(0x00020000)
const ATTR_SECURITY_CAPABILITIES: usize = 0x00020009;

const JOB_MEM_LIMIT: usize = 4 << 30; // 4 GiB
const JOB_PROC_LIMIT: u32 = 128;

const FILE_ALL_ACCESS: u32 = 0x1FFFFF | 0x10000000; // FILE_ALL_ACCESS|GENERIC_ALL
const FILE_READ_EXEC: u32 = 0x1200a9; // GENERIC_READ|GENERIC_EXECUTE + FILE read data etc.

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// Get (or create) the AppContainer profile SID. The profile MUST exist:
/// `DeriveAppContainerSidFromAppContainerName` hashes any name and always
/// succeeds, so a missing profile only surfaces later as a failed spawn.
fn app_container_sid() -> Result<PSID> {
    unsafe {
        match CreateAppContainerProfile(
            PCWSTR(wide(PROFILE_NAME).as_ptr()),
            PCWSTR(wide(PROFILE_DISPLAY).as_ptr()),
            PCWSTR(wide(PROFILE_DISPLAY).as_ptr()),
            None,
        ) {
            Ok(sid) => Ok(sid),
            // ERROR_ALREADY_EXISTS (0x800700B7): profile already registered —
            // derive the SID for it.
            Err(e) if e.code().0 == 0x800700B7u32 as i32 => {
                DeriveAppContainerSidFromAppContainerName(PCWSTR(wide(PROFILE_NAME).as_ptr()))
                    .map_err(|e| anyhow::anyhow!("DeriveAppContainerSid: {e}"))
            }
            Err(e) => Err(anyhow::anyhow!("CreateAppContainerProfile: {e}")),
        }
    }
}

/// Build a DACL = existing DACL of `path` + one allow ACE for `sid`.
/// Returns the new ACL pointer plus the SD to free afterwards.
unsafe fn merged_dacl(
    path: &Path,
    sid: PSID,
    mask: u32,
    inherit: bool,
) -> Result<(*mut ACL, PSECURITY_DESCRIPTOR)> {
    let wpath = wide(&path.to_string_lossy());
    let trustee = TRUSTEE_W {
        pMultipleTrustee: ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: PWSTR(sid.0 as *mut u16),
    };
    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: if inherit {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            windows::Win32::Security::ACE_FLAGS(0)
        },
        Trustee: trustee,
    };
    let mut old_dacl: *mut ACL = ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = PSECURITY_DESCRIPTOR(ptr::null_mut());
    let rc = GetNamedSecurityInfoW(
        PCWSTR(wpath.as_ptr()),
        SE_FILE_OBJECT,
        OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0),
        None,
        None,
        Some(&mut old_dacl),
        None,
        &mut sd,
    );
    if rc.0 != 0 {
        return Err(anyhow::anyhow!(
            "GetNamedSecurityInfoW {} -> {}",
            path.display(),
            rc.0
        ));
    }
    let mut new_dacl: *mut ACL = ptr::null_mut();
    let rc = SetEntriesInAclW(Some(&[ea]), Some(old_dacl), &mut new_dacl);
    if rc.0 != 0 {
        if !sd.0.is_null() {
            LocalFree(HLOCAL(sd.0));
        }
        return Err(anyhow::anyhow!("SetEntriesInAclW -> {}", rc.0));
    }
    Ok((new_dacl, sd))
}

/// Grant `sid` `mask` on `path` AND recursively on existing children —
/// inheritance flags only reach files created later, so the tree walk is
/// what actually opens existing content to the container.
unsafe fn grant_tree(path: &Path, sid: PSID, mask: u32) -> Result<()> {
    let wpath = wide(&path.to_string_lossy());
    let (new_dacl, sd) = merged_dacl(path, sid, mask, true)?;
    let rc = TreeSetNamedSecurityInfoW(
        PCWSTR(wpath.as_ptr()),
        SE_FILE_OBJECT,
        OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0),
        None,
        None,
        Some(new_dacl),
        None,
        TREE_SEC_INFO_SET,
        None,
        windows::Win32::Security::Authorization::PROG_INVOKE_SETTING(0),
        None,
    );
    if !new_dacl.is_null() {
        LocalFree(HLOCAL(new_dacl as _));
    }
    if !sd.0.is_null() {
        LocalFree(HLOCAL(sd.0));
    }
    if rc.0 != 0 {
        return Err(anyhow::anyhow!(
            "TreeSetNamedSecurityInfoW {} -> {}",
            path.display(),
            rc.0
        ));
    }
    Ok(())
}

/// Run `grant_tree` in a worker thread with a timeout. ACL APIs can stall on
/// junctions/placeholder files; a hung grant is abandoned (the thread leaks,
/// the exec continues) rather than blocking the sandbox.
fn grant_tree_bounded(path: &Path, sid: PSID, mask: u32, secs: u64) -> bool {
    let dir = path.to_path_buf();
    let sid_raw = sid.0 as usize;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let sid = PSID(sid_raw as *mut _);
        let _ = tx.send(grant_tree(&dir, sid, mask).is_ok());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .unwrap_or(false)
}

/// Grants persist in the file-system ACL — cache successes AND failures so
/// repeat `exec` calls don't pay the grant cost (or re-stall on a junction).
/// Stored under %LOCALAPPDATA%\cosmos-cli\zsandbox-grants.txt.
fn grants_cache_file() -> Option<PathBuf> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("cosmos-cli"))?;
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("zsandbox-grants.txt"))
}

fn grant_cached(path: &Path, sid: PSID, mask: u32, secs: u64) -> bool {
    let key = path.to_string_lossy().to_lowercase();
    if let Some(f) = grants_cache_file() {
        if let Ok(text) = fs::read_to_string(&f) {
            for line in text.lines() {
                if let Some((tag, p)) = line.split_once('|') {
                    if p == key {
                        return tag == "ok"; // cached result — skip re-grant
                    }
                }
            }
        }
    }
    let ok = grant_tree_bounded(path, sid, mask, secs);
    if let Some(f) = grants_cache_file() {
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&f)
            .map(|mut fd| {
                use std::io::Write;
                let _ = writeln!(fd, "{}|{}", if ok { "ok" } else { "fail" }, key);
            });
    }
    ok
}

/// Environment for the child: inherited minus anything that smells like a
/// credential, with TEMP/TMP pointed inside the workspace.
fn child_env(workspace: &Path) -> Vec<u16> {
    // Windows requires the env block sorted case-insensitively; uppercasing
    // keys gives BTreeMap's byte-order the same order and dedups case twins.
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in std::env::vars() {
        let ku = k.to_uppercase();
        if ku.ends_with("_API_KEY")
            || ku.ends_with("_TOKEN")
            || ku.ends_with("_SECRET")
            || ku.ends_with("_PASSWORD")
            || ku.contains("CREDENTIAL")
            || ku.contains("AUTH_TOKEN")
        {
            continue;
        }
        map.entry(ku).or_insert(v);
    }
    let tmp = workspace.join(".zsandbox-tmp");
    let _ = fs::create_dir_all(&tmp);
    map.insert("TEMP".into(), tmp.to_string_lossy().into());
    map.insert("TMP".into(), tmp.to_string_lossy().into());
    let mut out: Vec<u16> = Vec::new();
    for (k, v) in map {
        out.extend(wide(&format!("{k}={v}")));
    }
    out.push(0);
    out
}

/// Build the command line string with quoting.
fn command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') || a.is_empty() {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn make_job() -> Result<HANDLE> {
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null()).context("CreateJobObjectW")?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.BasicLimitInformation.ActiveProcessLimit = JOB_PROC_LIMIT;
        info.ProcessMemoryLimit = JOB_MEM_LIMIT;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .context("SetInformationJobObject")?;
        Ok(job)
    }
}

/// Bounded join: grandchildren may inherit the pipe write end and keep it
/// open after the root process dies — never block the caller on that.
fn collect(t: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(t.join().unwrap_or_default());
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_default()
}

/// Read a pipe to EOF on a thread. HANDLE is `*mut c_void` (not Send) — pass
/// the raw value and reconstruct inside the thread.
fn reader_thread(handle: HANDLE) -> std::thread::JoinHandle<Vec<u8>> {
    let raw = handle.0 as usize;
    std::thread::spawn(move || unsafe {
        let handle = HANDLE(raw as *mut _);
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let mut read = 0u32;
            let ok = ReadFile(handle, Some(&mut buf), Some(&mut read), None);
            if ok.is_err() || read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        let _ = CloseHandle(handle);
        out
    })
}

struct PipeEnds {
    read: HANDLE,
    write: HANDLE,
}

fn make_pipe() -> Result<PipeEnds> {
    unsafe {
        let mut r = HANDLE::default();
        let mut w = HANDLE::default();
        CreatePipe(&mut r, &mut w, None, 0).context("CreatePipe")?;
        Ok(PipeEnds { read: r, write: w })
    }
}

fn inherit(handle: HANDLE, on: bool) -> Result<()> {
    unsafe {
        SetHandleInformation(
            handle,
            HANDLE_FLAG_INHERIT.0,
            if on {
                HANDLE_FLAG_INHERIT
            } else {
                HANDLE_FLAGS(0)
            },
        )
        .context("SetHandleInformation")
    }
}

pub fn exec(opts: &ExecOpts) -> Result<ExecResult> {
    if opts.argv.is_empty() {
        bail!("zsandbox: empty argv");
    }
    let workspace = {
        let c = fs::canonicalize(&opts.workspace).unwrap_or_else(|_| opts.workspace.clone());
        // canonicalize yields \\?\ verbatim paths which break lpCurrentDirectory
        let s = c.to_string_lossy();
        match s.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => c,
        }
    };
    match exec_appcontainer(&workspace, opts) {
        Ok(r) => Ok(r),
        Err(e) => {
            let ac_err = format!("{e:#}");
            match exec_restricted(&workspace, opts) {
                Ok(mut r) => {
                    r.degraded = Some(format!("appcontainer_unavailable: {ac_err}"));
                    Ok(r)
                }
                // Last resort: run inside the Job only — timeout/memory/proc
                // limits still apply, but there is NO isolation. Honest degraded.
                Err(e2) => {
                    let mut r = exec_plain(&workspace, opts)?;
                    r.degraded = Some(format!(
                        "appcontainer_unavailable: {ac_err}; restricted_token_unavailable: {e2:#}; running with job limits only (no isolation)"
                    ));
                    Ok(r)
                }
            }
        }
    }
}

/// No isolation: plain process inside the Job (timeout/mem/proc caps still
/// enforced). Only reached when every sandbox layer failed.
fn exec_plain(workspace: &Path, opts: &ExecOpts) -> Result<ExecResult> {
    unsafe {
        let out_pipe = make_pipe()?;
        let err_pipe = make_pipe()?;
        let in_pipe = make_pipe()?;
        inherit(out_pipe.write, true)?;
        inherit(err_pipe.write, true)?;
        inherit(in_pipe.read, true)?;

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: in_pipe.read,
            hStdOutput: out_pipe.write,
            hStdError: err_pipe.write,
            ..Default::default()
        };

        let app = resolve_exe(&opts.argv[0]).unwrap_or_else(|| PathBuf::from(&opts.argv[0]));
        let wapp = wide(&app.to_string_lossy());
        let mut wcmd = wide(&command_line(&opts.argv));
        let env = child_env(workspace);
        let wdir = wide(&workspace.to_string_lossy());
        let mut pi = PROCESS_INFORMATION::default();
        let created = CreateProcessW(
            PCWSTR(wapp.as_ptr()),
            PWSTR(wcmd.as_mut_ptr()),
            None,
            None,
            true,
            CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED | DETACHED_PROCESS | CREATE_NO_WINDOW,
            Some(env.as_ptr() as *const _),
            PCWSTR(wdir.as_ptr()),
            &si,
            &mut pi,
        );
        let _ = CloseHandle(out_pipe.write);
        let _ = CloseHandle(err_pipe.write);
        let _ = CloseHandle(in_pipe.read);
        let _ = CloseHandle(in_pipe.write);
        if let Err(e) = created {
            return Err(anyhow::anyhow!("CreateProcessW (plain): {e}"));
        }
        finish_run(pi, opts, out_pipe.read, err_pipe.read, None)
    }
}

fn dbg(msg: &str) {
    if std::env::var_os("ZSANDBOX_DEBUG").is_some() {
        eprintln!("zsandbox-debug: {msg}");
    }
}

fn exec_appcontainer(workspace: &Path, opts: &ExecOpts) -> Result<ExecResult> {
    // Serialize the setup phase: concurrent CreateAppContainerProfile /
    // grant calls from threads racing in the same process can observe a
    // half-created profile and fail the spawn. Child run time stays parallel.
    static SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _setup = SETUP.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        dbg("resolving appcontainer sid");
        let sid = app_container_sid()?;
        let tmp = workspace.join(".zsandbox-tmp");
        fs::create_dir_all(&tmp)?;

        // The workspace is a hard requirement — the child must be able to
        // write there. Recursive grant so existing files are covered.
        dbg("granting workspace (recursive)");
        if !grant_tree_bounded(workspace, sid, FILE_ALL_ACCESS, 60) {
            return Err(anyhow::anyhow!(
                "could not grant AppContainer access to workspace {}",
                workspace.display()
            ));
        }
        // Read+execute grants for tool directories the child may need.
        // System dirs (C:\Windows, Program Files) already grant Everyone RX —
        // user-profile dirs do not. Each grant is time-boxed: junctions and
        // placeholder files can stall the ACL APIs indefinitely.
        for dir in grant_dirs(&opts.argv[0]) {
            dbg(&format!("grant {}", dir.display()));
            let ok = grant_cached(&dir, sid, FILE_READ_EXEC, 10);
            dbg(&format!("grant {} -> {}", dir.display(), ok));
        }
        dbg("grants done");

        let mut caps: Vec<SID_AND_ATTRIBUTES> = Vec::new();
        let mut inet_sid = PSID::default();
        if opts.net == NetPolicy::Allow {
            ConvertStringSidToSidW(PCWSTR(wide(SID_INTERNET_CLIENT).as_ptr()), &mut inet_sid)
                .context("ConvertStringSidToSidW")?;
            caps.push(SID_AND_ATTRIBUTES {
                Sid: inet_sid,
                Attributes: 0x4, // SE_GROUP_ENABLED
            });
        }
        let mut sec = SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            Capabilities: if caps.is_empty() {
                ptr::null_mut()
            } else {
                caps.as_mut_ptr()
            },
            CapabilityCount: caps.len() as u32,
            Reserved: 0,
        };

        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(
            LPPROC_THREAD_ATTRIBUTE_LIST(ptr::null_mut()),
            1,
            0,
            &mut size,
        );
        let mut buf = vec![0u8; size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buf.as_mut_ptr() as _);
        InitializeProcThreadAttributeList(list, 1, 0, &mut size)
            .context("InitializeProcThreadAttributeList")?;
        UpdateProcThreadAttribute(
            list,
            0,
            ATTR_SECURITY_CAPABILITIES,
            Some(&mut sec as *mut _ as *const _),
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
            None,
            None,
        )
        .context("UpdateProcThreadAttribute")?;

        let out_pipe = make_pipe()?;
        let err_pipe = make_pipe()?;
        let in_pipe = make_pipe()?;
        inherit(out_pipe.write, true)?;
        inherit(err_pipe.write, true)?;
        inherit(in_pipe.read, true)?;

        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = in_pipe.read;
        si.StartupInfo.hStdOutput = out_pipe.write;
        si.StartupInfo.hStdError = err_pipe.write;
        si.lpAttributeList = list;

        let app = resolve_exe(&opts.argv[0]).unwrap_or_else(|| PathBuf::from(&opts.argv[0]));
        let wapp = wide(&app.to_string_lossy());
        let mut wcmd = wide(&command_line(&opts.argv));
        let env = child_env(workspace);
        let wdir = wide(&workspace.to_string_lossy());
        let mut pi = PROCESS_INFORMATION::default();
        dbg(&format!(
            "CreateProcessW app={} cmd={}",
            app.display(),
            String::from_utf16_lossy(&wcmd)
        ));
        let created = CreateProcessW(
            PCWSTR(wapp.as_ptr()),
            PWSTR(wcmd.as_mut_ptr()),
            None,
            None,
            true,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_SUSPENDED
                | DETACHED_PROCESS
                | CREATE_NO_WINDOW,
            Some(env.as_ptr() as *const _),
            PCWSTR(wdir.as_ptr()),
            &si.StartupInfo as *const STARTUPINFOW,
            &mut pi,
        );
        DeleteProcThreadAttributeList(list);
        let _ = CloseHandle(out_pipe.write);
        let _ = CloseHandle(err_pipe.write);
        let _ = CloseHandle(in_pipe.read);
        let _ = CloseHandle(in_pipe.write);
        match &created {
            Ok(()) => dbg(&format!("spawned pid={}", pi.dwProcessId)),
            Err(e) => dbg(&format!("CreateProcessW failed: {e}")),
        }
        if let Err(e) = created {
            return Err(anyhow::anyhow!("CreateProcessW (appcontainer): {e}"));
        }
        finish_run(pi, opts, out_pipe.read, err_pipe.read, None)
    }
}

/// Where the child may need read/execute beyond what `Everyone` already
/// covers: the executable's own directory (and a Git-for-Windows install
/// root if detected), PATH entries under %USERPROFILE% (not Everyone-granted
/// by default), and anything in ZSANDBOX_EXTRA_RX.
fn grant_dirs(exe: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |d: PathBuf| {
        if d.is_dir()
            && !dirs.iter().any(|x| {
                x.to_string_lossy()
                    .eq_ignore_ascii_case(&d.to_string_lossy())
            })
        {
            dirs.push(d);
        }
    };
    if let Some(resolved) = resolve_exe(exe) {
        if let Some(p) = resolved.parent() {
            push(p.to_path_buf());
            if let Some(gp) = p.parent().and_then(|g| g.parent()) {
                if gp.to_string_lossy().contains("Git") {
                    push(gp.to_path_buf());
                }
            }
        }
    } else if let Some(p) = PathBuf::from(exe).parent() {
        push(p.to_path_buf());
    }
    let profile = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|p| p.to_string_lossy().to_lowercase());
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            if let Some(up) = &profile {
                if d.to_string_lossy().to_lowercase().starts_with(up) {
                    push(d);
                }
            }
        }
    }
    if let Ok(extra) = std::env::var("ZSANDBOX_EXTRA_RX") {
        for d in extra.split(';').filter(|s| !s.is_empty()) {
            push(PathBuf::from(d));
        }
    }
    dirs
}

/// Resolve `exe` against PATH (PATHEXT-aware enough for our use).
fn resolve_exe(exe: &str) -> Option<PathBuf> {
    let p = PathBuf::from(exe);
    if p.is_file() {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for cand in [
            exe.to_string(),
            format!("{exe}.exe"),
            format!("{exe}.cmd"),
            format!("{exe}.bat"),
        ] {
            let f = dir.join(&cand);
            if f.is_file() {
                return Some(f);
            }
        }
    }
    None
}

fn exec_restricted(workspace: &Path, opts: &ExecOpts) -> Result<ExecResult> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_QUERY
                | TOKEN_QUERY_SOURCE
                | TOKEN_ADJUST_DEFAULT,
            &mut token,
        )
        .context("OpenProcessToken")?;
        let mut restricted = HANDLE::default();
        CreateRestrictedToken(
            token,
            DISABLE_MAX_PRIVILEGE,
            None,
            None,
            None,
            &mut restricted,
        )
        .context("CreateRestrictedToken")?;
        let _ = CloseHandle(token);
        let mut il_sid = PSID::default();
        ConvertStringSidToSidW(PCWSTR(wide(SID_LOW_INTEGRITY).as_ptr()), &mut il_sid)
            .context("ConvertStringSidToSidW")?;
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: il_sid,
                Attributes: SE_GROUP_INTEGRITY,
            },
        };
        SetTokenInformation(
            restricted,
            TokenIntegrityLevel,
            &label as *const _ as *const _,
            std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        )
        .context("SetTokenInformation(TokenIntegrityLevel)")?;

        let out_pipe = make_pipe()?;
        let err_pipe = make_pipe()?;
        let in_pipe = make_pipe()?;
        inherit(out_pipe.write, true)?;
        inherit(err_pipe.write, true)?;
        inherit(in_pipe.read, true)?;

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: in_pipe.read,
            hStdOutput: out_pipe.write,
            hStdError: err_pipe.write,
            ..Default::default()
        };

        let mut wcmd = wide(&command_line(&opts.argv));
        let env = child_env(workspace);
        let wdir = wide(&workspace.to_string_lossy());
        let mut pi = PROCESS_INFORMATION::default();
        let created = CreateProcessWithTokenW(
            restricted,
            LOGON_WITH_PROFILE,
            PCWSTR::null(),
            PWSTR(wcmd.as_mut_ptr()),
            CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED | DETACHED_PROCESS | CREATE_NO_WINDOW,
            Some(env.as_ptr() as *const _),
            PCWSTR(wdir.as_ptr()),
            &si,
            &mut pi,
        );
        let _ = CloseHandle(out_pipe.write);
        let _ = CloseHandle(err_pipe.write);
        let _ = CloseHandle(in_pipe.read);
        let _ = CloseHandle(in_pipe.write);
        let _ = CloseHandle(restricted);
        if let Err(e) = created {
            return Err(anyhow::anyhow!("CreateProcessWithTokenW: {e}"));
        }
        finish_run(pi, opts, out_pipe.read, err_pipe.read, None)
    }
}

/// Common tail: assign to Job, wait with timeout, collect pipes, exit code.
fn finish_run(
    pi: PROCESS_INFORMATION,
    opts: &ExecOpts,
    out_read: HANDLE,
    err_read: HANDLE,
    degraded: Option<String>,
) -> Result<ExecResult> {
    unsafe {
        let job = make_job()?;
        // Best-effort: nested-job restrictions can deny this; the AppContainer
        // still sandboxes the child and the timeout path kills the root pid.
        let job_ok = AssignProcessToJobObject(job, pi.hProcess).is_ok();
        // child was created suspended: release it only after the job assign
        let resumed = ResumeThread(pi.hThread);
        if std::env::var_os("ZSANDBOX_DEBUG").is_some() {
            eprintln!(
                "zsandbox-debug: job_assign={job_ok} resume={resumed} pid={}",
                pi.dwProcessId
            );
        }
        let out_thread = reader_thread(out_read);
        let err_thread = reader_thread(err_read);

        let ms = opts.timeout.as_millis().min(u32::MAX as u128) as u32;
        let wait = WaitForSingleObject(pi.hProcess, ms);
        let mut timed_out = false;
        if wait == WAIT_TIMEOUT {
            timed_out = true;
            let _ = TerminateJobObject(job, 124);
            // If the process never joined the job (nested-job restrictions),
            // the terminate above is a no-op — kill the root directly so the
            // pipe readers can observe EOF and join() cannot hang.
            let _ = WaitForSingleObject(pi.hProcess, 3000);
            let _ = TerminateProcess(pi.hProcess, 124);
            let _ = WaitForSingleObject(pi.hProcess, 3000);
        }
        let mut code = 0u32;
        let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        let stdout = collect(out_thread);
        let stderr = collect(err_thread);
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(job);
        Ok(ExecResult {
            code: if timed_out { 124 } else { code as i32 },
            stdout,
            stderr,
            timed_out,
            degraded,
        })
    }
}
