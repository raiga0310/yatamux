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

/// 指定 PID のプロセスが現在いる作業ディレクトリを返す。
///
/// - `NtQueryInformationProcess` (ntdll.dll) でプロセスの PEB アドレスを取得し、
///   `ReadProcessMemory` で PEB → ProcessParameters.CurrentDirectory を読み取る。
/// - プロセスが終了済み・権限不足の場合は `None` を返す。
/// - Windows 以外では常に `None` を返すスタブ。
#[cfg(windows)]
pub fn find_process_cwd(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
        .ok()?;

        let result = read_process_cwd_inner(handle);
        let _ = CloseHandle(handle);
        result
    }
}

/// ハンドルを受け取り cwd を読み取る内部実装（ハンドルのクローズは呼び出し元が行う）
#[cfg(windows)]
unsafe fn read_process_cwd_inner(
    handle: windows::Win32::Foundation::HANDLE,
) -> Option<String> {
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows::core::s;

    type FnNtQueryInformationProcess = unsafe extern "system" fn(
        process_handle: windows::Win32::Foundation::HANDLE,
        process_information_class: u32,
        process_information: *mut std::ffi::c_void,
        process_information_length: u32,
        return_length: *mut u32,
    ) -> i32;

    // ntdll から NtQueryInformationProcess をロード
    let ntdll = LoadLibraryA(s!("ntdll.dll")).ok()?;
    let fn_ptr = GetProcAddress(ntdll, s!("NtQueryInformationProcess"))?;
    let nt_query: FnNtQueryInformationProcess = std::mem::transmute(fn_ptr);

    // ProcessBasicInformation (class=0)
    // x64 レイアウト: NTSTATUS(4)+pad(4)+PEB*(8)+AffinityMask(8)+BasePriority(8)+UniqueProcessId(8)+InheritedFrom(8) = 48 bytes
    let mut pbi = [0u8; 48];
    let mut ret_len = 0u32;
    let status = nt_query(
        handle,
        0,
        pbi.as_mut_ptr() as *mut std::ffi::c_void,
        pbi.len() as u32,
        &mut ret_len,
    );
    if status != 0 {
        return None;
    }
    // PebBaseAddress は offset 8 (u64)
    let peb_addr = u64::from_ne_bytes(pbi[8..16].try_into().ok()?);
    if peb_addr == 0 {
        return None;
    }

    // PEB の先頭 0x28 bytes を読む（ProcessParameters ポインタは offset 0x20）
    let mut peb_buf = [0u8; 0x28];
    ReadProcessMemory(
        handle,
        peb_addr as *const std::ffi::c_void,
        peb_buf.as_mut_ptr() as *mut std::ffi::c_void,
        peb_buf.len(),
        None,
    )
    .ok()?;
    let proc_params_addr = u64::from_ne_bytes(peb_buf[0x20..0x28].try_into().ok()?);
    if proc_params_addr == 0 {
        return None;
    }

    // RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath (offset 0x38)
    // UNICODE_STRING レイアウト: Length(u16) + MaximumLength(u16) + _pad(u32) + Buffer(u64) = 16 bytes
    let mut curdir_buf = [0u8; 16];
    ReadProcessMemory(
        handle,
        (proc_params_addr + 0x38) as *const std::ffi::c_void,
        curdir_buf.as_mut_ptr() as *mut std::ffi::c_void,
        curdir_buf.len(),
        None,
    )
    .ok()?;
    let path_len = u16::from_ne_bytes(curdir_buf[0..2].try_into().ok()?) as usize;
    let path_buf_addr = u64::from_ne_bytes(curdir_buf[8..16].try_into().ok()?);
    if path_len == 0 || path_buf_addr == 0 {
        return None;
    }

    // UTF-16 LE パス文字列を読み取る
    let mut path_bytes = vec![0u8; path_len];
    ReadProcessMemory(
        handle,
        path_buf_addr as *const std::ffi::c_void,
        path_bytes.as_mut_ptr() as *mut std::ffi::c_void,
        path_len,
        None,
    )
    .ok()?;

    let utf16: Vec<u16> = path_bytes
        .chunks_exact(2)
        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
        .collect();
    let path = String::from_utf16_lossy(&utf16);
    // 末尾の '\' を除去（"C:\foo\" → "C:\foo"）
    Some(path.trim_end_matches('\\').to_string())
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
