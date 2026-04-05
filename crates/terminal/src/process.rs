//! プロセスツリー走査ユーティリティ
//!
//! ConPTY の子プロセス PID を親として持つ孫プロセス（実際に動いているコマンド）を
//! Windows の ToolHelp API で特定する。

/// シェルとして除外する既知のプロセス名（小文字、.exe なし）
const KNOWN_SHELLS: &[&str] = &["cmd", "powershell", "pwsh", "bash", "sh"];

/// 指定 PID の子孫プロセスのうち、既知シェル以外の最初のプロセス名を返す。
///
/// - ConPTY の直接子（cmd.exe / PowerShell 等）は `KNOWN_SHELLS` リストで除外する。
/// - 孫プロセス（claude.exe など）が見つかれば `.exe` を除いた名前を返す。
/// - Windows 以外のプラットフォームでは常に `None` を返すスタブ。
#[cfg(windows)]
pub fn find_active_command(parent_pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
    };

    // プロセス全体のスナップショットを取得
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()? };

    let mut entry = PROCESSENTRY32 {
        dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
        ..Default::default()
    };

    // 全プロセスを列挙して parent_pid を th32ParentProcessID に持つものを収集
    let mut children: Vec<u32> = Vec::new();
    {
        let ok = unsafe { Process32First(snapshot, &mut entry) };
        if ok.is_ok() {
            loop {
                if entry.th32ParentProcessID == parent_pid {
                    children.push(entry.th32ProcessID);
                }
                if unsafe { Process32Next(snapshot, &mut entry) }.is_err() {
                    break;
                }
            }
        }
    }

    // 子プロセスの子（孫プロセス）も同様に収集し、シェル以外のコマンドを返す
    // まず直接の子のプロセス名を確認し、シェル以外なら返す
    // シェルなら再度スナップショットを走査して孫を探す

    // スナップショットを再利用するため、全エントリをメモリに収める
    let mut all_entries: Vec<(u32, u32, String)> = Vec::new(); // (pid, ppid, name)
    {
        let mut e = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };
        if unsafe { Process32First(snapshot, &mut e) }.is_ok() {
            loop {
                let name = {
                    let raw = &e.szExeFile;
                    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                    let bytes: Vec<u8> = raw[..end].iter().map(|&c| c as u8).collect();
                    String::from_utf8_lossy(&bytes).to_lowercase()
                };
                all_entries.push((e.th32ProcessID, e.th32ParentProcessID, name));
                if unsafe { Process32Next(snapshot, &mut e) }.is_err() {
                    break;
                }
            }
        }
    }

    unsafe { CloseHandle(snapshot).ok() };

    // BFS でプロセスツリーを走査し、シェル以外のコマンドを最初に返す
    let mut queue: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    queue.push_back(parent_pid);

    while let Some(pid) = queue.pop_front() {
        for &(child_pid, ppid, ref name) in &all_entries {
            if ppid != pid {
                continue;
            }
            // .exe 拡張子を除去
            let base_name = name.trim_end_matches(".exe");

            if KNOWN_SHELLS.contains(&base_name) {
                // シェルは除外してさらに子を探す
                queue.push_back(child_pid);
            } else {
                // シェル以外のプロセスが見つかった
                return Some(base_name.to_string());
            }
        }
    }

    None
}

/// Windows 以外プラットフォーム用スタブ
#[cfg(not(windows))]
pub fn find_active_command(_parent_pid: u32) -> Option<String> {
    None
}

/// 指定 PID のプロセスの現在の作業ディレクトリを返す。
///
/// NtQueryInformationProcess（ntdll.dll を動的ロード）→ PEB →
/// ReadProcessMemory で RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath
/// （offset 0x38、UTF-16 LE）を読み取り、末尾の `\` を除去して返す。
///
/// Windows 以外のプラットフォームでは常に `None` を返すスタブ。
#[cfg(windows)]
pub fn find_process_cwd(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    // NtQueryInformationProcess の関数ポインタ型
    #[repr(C)]
    struct ProcessBasicInformation {
        reserved1: usize,
        peb_base_address: usize,
        reserved2: [usize; 2],
        unique_process_id: usize,
        reserved3: usize,
    }

    type NtQueryFn = unsafe extern "system" fn(
        HANDLE,
        u32,
        *mut std::ffi::c_void,
        u32,
        *mut u32,
    ) -> i32;

    let handle = unsafe {
        OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?
    };

    let result = (|| -> Option<String> {
        // ntdll.dll を動的ロードして NtQueryInformationProcess を取得
        let ntdll =
            unsafe { LoadLibraryA(windows::core::PCSTR(b"ntdll.dll\0".as_ptr())).ok()? };
        let fn_ptr = unsafe {
            GetProcAddress(
                ntdll,
                windows::core::PCSTR(b"NtQueryInformationProcess\0".as_ptr()),
            )?
        };
        let nt_query: NtQueryFn = unsafe { std::mem::transmute(fn_ptr) };

        // ProcessBasicInformation (class 0) で PEB アドレスを取得
        let mut pbi = ProcessBasicInformation {
            reserved1: 0,
            peb_base_address: 0,
            reserved2: [0; 2],
            unique_process_id: 0,
            reserved3: 0,
        };
        let mut ret_len = 0u32;
        let status = unsafe {
            nt_query(
                handle,
                0,
                &mut pbi as *mut _ as *mut _,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                &mut ret_len,
            )
        };
        if status != 0 || pbi.peb_base_address == 0 {
            return None;
        }
        let peb_addr = pbi.peb_base_address as u64;

        // PEB + 0x20 → ProcessParameters ポインタ（64-bit）
        let mut proc_params_ptr: u64 = 0;
        unsafe {
            ReadProcessMemory(
                handle,
                (peb_addr + 0x20) as *const _,
                &mut proc_params_ptr as *mut _ as *mut _,
                8,
                None,
            )
            .ok()?;
        }
        if proc_params_ptr == 0 {
            return None;
        }

        // RTL_USER_PROCESS_PARAMETERS + 0x38 → CurrentDirectory.DosPath.Length (u16)
        let mut length: u16 = 0;
        unsafe {
            ReadProcessMemory(
                handle,
                (proc_params_ptr + 0x38) as *const _,
                &mut length as *mut _ as *mut _,
                2,
                None,
            )
            .ok()?;
        }
        if length == 0 {
            return None;
        }

        // CurrentDirectory.DosPath.Buffer は UNICODE_STRING の +8 バイト目（64-bit）
        let mut buffer_ptr: u64 = 0;
        unsafe {
            ReadProcessMemory(
                handle,
                (proc_params_ptr + 0x38 + 8) as *const _,
                &mut buffer_ptr as *mut _ as *mut _,
                8,
                None,
            )
            .ok()?;
        }
        if buffer_ptr == 0 {
            return None;
        }

        // UTF-16 LE の文字列を読み取る
        let num_chars = (length / 2) as usize;
        let mut buf = vec![0u16; num_chars];
        unsafe {
            ReadProcessMemory(
                handle,
                buffer_ptr as *const _,
                buf.as_mut_ptr() as *mut _,
                num_chars * 2,
                None,
            )
            .ok()?;
        }

        let s = String::from_utf16_lossy(&buf);
        Some(s.trim_end_matches('\\').to_string())
    })();

    unsafe { CloseHandle(handle).ok() };
    result
}

/// Windows 以外プラットフォーム用スタブ
#[cfg(not(windows))]
pub fn find_process_cwd(_pid: u32) -> Option<String> {
    None
}

// ── テスト ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 自プロセスの PID を渡したとき、パニックしないことを確認する。
    /// Windows でなければ None を返す。
    #[test]
    fn test_find_active_command_does_not_panic() {
        let pid = std::process::id();
        // パニックしなければ OK（None でも Some でも問題なし）
        let _ = find_active_command(pid);
    }

    /// 存在しない PID（u32::MAX）を渡しても None を返す
    #[test]
    fn test_find_active_command_unknown_pid_returns_none() {
        let result = find_active_command(u32::MAX);
        assert!(result.is_none());
    }
}
