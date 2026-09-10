use crate::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `contents` to `path` atomically, readable only by the owner.
///
/// The bytes go to a sibling temporary file that is restricted to the account
/// running the process before a single byte is written, then flushed to disk
/// and renamed over the target. Three properties fall out of that:
///
/// * A crash or power loss leaves either the previous file or the new one. A
///   plain `fs::write` truncates first, so an interrupted save can leave a
///   half-written file, and for `vault.json` that means losing every stored key.
/// * The finished file is never briefly readable by anyone else, and a file
///   left behind with looser permissions by an earlier version is replaced by a
///   restricted one on the next save. `rename` carries the temporary file's
///   permissions over to the target on both platforms.
/// * The restriction is applied on the open handle rather than by path, so no
///   other process can substitute a different file in between.
///
/// The mechanism differs per platform because the permission models do:
///
/// * Unix creates the temporary file with mode 0600.
/// * Windows has no mode bits, so the temporary file gets an explicit protected
///   DACL naming only the account that runs the process. The `owner_only`
///   module documents why that is more than the ACL the user profile already
///   supplies.
///
/// A filesystem that cannot hold the restriction fails the whole save rather
/// than falling back to an unprotected write. `vault.json` is the only place
/// private keys are persisted, so a store that cannot keep it to one account is
/// a store this refuses to use; the previous file is left intact and the error
/// reaches the caller.
pub(crate) fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = temp_path(path);
    match write_temp(&tmp, contents).and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

/// Result of unlinking a private file. A directory-sync failure is reported
/// separately because the irreversible unlink has already happened and callers
/// must still clear any in-memory state that could authorize the deleted data.
pub(crate) struct RemovePrivateOutcome {
    pub(crate) durability_error: Option<std::io::Error>,
}

/// Remove an atomically-written private file and any crash-leftover sibling
/// temporary copy. The temporary copy is deleted first so a failure there never
/// destroys the authoritative file while leaving an older encrypted snapshot
/// behind. Once unlinking succeeds, a containing-directory sync failure is
/// returned in the outcome rather than rewinding the operation: the target is
/// already gone and higher layers must cross their destructive reset boundary.
pub(crate) fn remove_private(path: &Path) -> Result<RemovePrivateOutcome> {
    remove_private_with_sync(path, sync_parent_directory)
}

fn remove_private_with_sync<F>(path: &Path, sync_parent: F) -> Result<RemovePrivateOutcome>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let removed_tmp = remove_if_present(&temp_path(path))?;
    let removed_target = remove_if_present(path)?;
    let durability_error = if removed_tmp || removed_target {
        sync_parent(path).err()
    } else {
        None
    };

    Ok(RemovePrivateOutcome { durability_error })
}

fn remove_if_present(path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)?.sync_all()
}

// Rust's standard library does not expose a portable directory fsync primitive
// on Windows. Keep the unlink behavior unchanged there rather than pretending a
// file handle sync makes the parent directory entry durable.
#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn write_temp(tmp: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Rewriting the DACL of the open handle needs WRITE_DAC, which
        // GENERIC_WRITE does not carry. Asking for it up front keeps the whole
        // operation on one handle, so the file that ends up restricted is
        // provably the file that is about to be written.
        options.access_mode(
            windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE
                | windows_sys::Win32::Storage::FileSystem::WRITE_DAC,
        );
    }

    let mut file = options.open(tmp)?;
    // The file is empty until this returns, so a DACL the parent directory
    // supplied never covers any of the contents.
    #[cfg(windows)]
    owner_only::restrict_to_owner(&file)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// Windows has no mode bits, and a file created under the user profile simply
/// inherits whatever the profile grants. That is per-user, but it is not
/// owner-only: `%APPDATA%` routinely carries inherited grants the account
/// holder never chose — an app-capability SID with full control over the whole
/// `AppData` tree is a real, observable example — and any future inheritable
/// ACE added anywhere above the app data directory lands on the vault too.
///
/// Replacing the inherited DACL with a protected one closes that off: the file
/// names exactly one account and inherits nothing.
#[cfg(windows)]
mod owner_only {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, GetLengthSid, GetTokenInformation, InitializeAcl, TokenUser,
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Give `file` a protected DACL whose only entry grants the account running
    /// this process full access to it.
    ///
    /// SYSTEM and the local administrators are deliberately not named. Both
    /// already hold `SeBackupPrivilege` and `SeTakeOwnershipPrivilege`, so an
    /// ACE for either grants nothing they cannot reach anyway, and leaving them
    /// out matches what Windows itself puts on the per-user DPAPI master key
    /// directories that hold comparable secrets.
    pub(super) fn restrict_to_owner(file: &File) -> io::Result<()> {
        let owner = OwnerSid::current()?;
        let acl = single_ace_dacl(owner.as_psid())?;

        let status = unsafe {
            SetSecurityInfo(
                file.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl.as_ptr(),
                std::ptr::null(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(())
    }

    /// An ACL is a variable-length structure with alignment requirements the
    /// API does not check, so it is built inside a `u32` buffer rather than a
    /// byte buffer.
    struct Dacl {
        buffer: Vec<u32>,
    }

    impl Dacl {
        fn as_ptr(&self) -> *const ACL {
            self.buffer.as_ptr().cast()
        }
    }

    /// Build a DACL holding one access-allowed ACE for `sid`.
    fn single_ace_dacl(sid: PSID) -> io::Result<Dacl> {
        // `ACCESS_ALLOWED_ACE` declares the SID as a single `u32` placeholder
        // that stands for the whole variable-length SID, so that word is
        // subtracted back out before the real length is added.
        let sid_len = unsafe { GetLengthSid(sid) } as usize;
        let acl_len = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            - std::mem::size_of::<u32>()
            + sid_len;

        let mut buffer = vec![0u32; acl_len.div_ceil(std::mem::size_of::<u32>())];
        let acl = buffer.as_mut_ptr().cast::<ACL>();

        if unsafe { InitializeAcl(acl, acl_len as u32, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) } == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Dacl { buffer })
    }

    /// The `TOKEN_USER` of the process token. The SID it exposes points into
    /// the buffer, so the buffer has to outlive every use of that pointer.
    pub(super) struct OwnerSid {
        buffer: Vec<u64>,
    }

    impl OwnerSid {
        pub(super) fn current() -> io::Result<Self> {
            let token = ProcessToken::open()?;

            // Sizing call. It always fails, and only `needed` is of interest.
            let mut needed: u32 = 0;
            unsafe {
                GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            }
            if needed == 0 {
                return Err(io::Error::last_os_error());
            }

            let mut buffer = vec![0u64; (needed as usize).div_ceil(std::mem::size_of::<u64>())];
            let ok = unsafe {
                GetTokenInformation(
                    token.0,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    needed,
                    &mut needed,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(Self { buffer })
        }

        pub(super) fn as_psid(&self) -> PSID {
            // SAFETY: the buffer holds a `TOKEN_USER` written by
            // `GetTokenInformation`, whose `Sid` points into that same buffer.
            unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    struct ProcessToken(HANDLE);

    impl ProcessToken {
        fn open() -> io::Result<Self> {
            let mut handle: HANDLE = std::ptr::null_mut();
            let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(handle))
        }
    }

    impl Drop for ProcessToken {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_private, remove_private_with_sync, write_private};

    #[test]
    fn writes_and_then_replaces_the_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        write_private(&path, "{\"first\":true}").expect("first write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"first\":true}");

        write_private(&path, "{\"second\":true}").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"second\":true}");
    }

    #[test]
    fn creates_the_parent_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("state.json");

        write_private(&path, "{}").expect("write");
        assert!(path.exists());
    }

    #[test]
    fn leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        write_private(&path, "{}").expect("write");

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["state.json".to_string()]);
    }

    #[test]
    fn remove_private_deletes_authoritative_and_crash_temp_copies() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        let tmp = dir.path().join("vault.json.tmp");
        std::fs::write(&path, "current ciphertext").expect("seed vault");
        std::fs::write(&tmp, "older ciphertext").expect("seed temp");

        remove_private(&path).expect("remove private file");

        assert!(!path.exists());
        assert!(!tmp.exists());
    }

    #[test]
    fn remove_private_reports_sync_failure_after_unlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");
        std::fs::write(&path, "ciphertext").expect("seed vault");

        let outcome = remove_private_with_sync(&path, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "directory sync failed",
            ))
        })
        .expect("unlink stage");

        assert!(!path.exists());
        assert!(outcome.durability_error.is_some());
    }

    #[test]
    fn remove_private_is_idempotent_when_files_are_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vault.json");

        remove_private(&path).expect("remove missing private file");
    }

    #[test]
    fn remove_private_is_idempotent_when_parent_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing").join("vault.json");

        remove_private(&path).expect("remove missing private file");
    }

    #[cfg(unix)]
    #[test]
    fn the_written_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        // An existing world-readable file must not keep its mode.
        std::fs::write(&path, "{}").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");

        write_private(&path, "{}").expect("write");

        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "unexpected mode {:o}", mode & 0o777);
    }
}

/// The Windows counterpart of `the_written_file_is_owner_only`.
///
/// Windows offers no portable way to ask "could another account read this?"
/// from inside a test process running as a single account, so these assert on
/// the access-control data itself: the exact set of entries on the file, and
/// the fact that an entry the parent directory hands out does not reach it.
/// The second is the behavioral half — a grant to a well-known non-owner group
/// is placed one level up and shown not to arrive.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::owner_only::OwnerSid;
    use super::write_private;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        AddAccessAllowedAceEx, CreateWellKnownSid, EqualSid, GetAce, GetLengthSid,
        GetSecurityDescriptorControl, InitializeAcl, WinWorldSid, ACCESS_ALLOWED_ACE, ACL,
        ACL_REVISION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, PSID,
        SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    /// `ACCESS_ALLOWED_ACE_TYPE`. It lives in a `windows-sys` module this crate
    /// does not enable, which carries a quarter of a megabyte of unrelated
    /// constants for this one byte.
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    #[test]
    fn the_written_file_names_only_the_current_account() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        write_private(&path, "{}").expect("write");

        let acl = FileDacl::read(&path);
        assert!(acl.is_protected, "the DACL still inherits from its parent");
        assert_eq!(acl.entries.len(), 1, "unexpected entries: {:?}", acl.entries);

        let entry = &acl.entries[0];
        assert_eq!(entry.ace_type, ACCESS_ALLOWED_ACE_TYPE);
        assert_eq!(entry.mask, FILE_ALL_ACCESS);

        let owner = OwnerSid::current().expect("token user");
        assert!(
            sids_match(entry.sid_bytes.as_slice(), owner.as_psid()),
            "the single entry does not name the account that wrote the file",
        );
    }

    #[test]
    fn an_inheritable_grant_on_the_directory_does_not_reach_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let everyone = well_known_sid(WinWorldSid);
        grant_full_control(dir.path(), sid_ptr(&everyone));

        let path = dir.path().join("state.json");
        write_private(&path, "{}").expect("write");

        let acl = FileDacl::read(&path);
        assert_eq!(acl.entries.len(), 1, "unexpected entries: {:?}", acl.entries);
        assert!(
            !sids_match(acl.entries[0].sid_bytes.as_slice(), sid_ptr(&everyone)),
            "the inheritable grant reached the file",
        );
    }

    #[test]
    fn an_existing_file_with_a_wider_dacl_is_narrowed_on_the_next_save() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{}").expect("seed");

        let everyone = well_known_sid(WinWorldSid);
        grant_full_control(&path, sid_ptr(&everyone));
        assert!(
            FileDacl::read(&path)
                .entries
                .iter()
                .any(|entry| sids_match(entry.sid_bytes.as_slice(), sid_ptr(&everyone))),
            "the test did not manage to widen the seeded file",
        );

        write_private(&path, "{}").expect("write");

        let acl = FileDacl::read(&path);
        assert!(acl.is_protected);
        assert_eq!(acl.entries.len(), 1, "unexpected entries: {:?}", acl.entries);
        assert!(!sids_match(
            acl.entries[0].sid_bytes.as_slice(),
            sid_ptr(&everyone)
        ));
    }

    #[derive(Debug)]
    struct AceEntry {
        ace_type: u8,
        mask: u32,
        sid_bytes: Vec<u8>,
    }

    struct FileDacl {
        is_protected: bool,
        entries: Vec<AceEntry>,
    }

    impl FileDacl {
        /// Copy every entry out of `path`'s DACL. The entries are copied rather
        /// than borrowed so the security descriptor can be freed here instead
        /// of leaking into every assertion.
        fn read(path: &Path) -> Self {
            let wide = wide(path);
            let mut acl: *mut ACL = std::ptr::null_mut();
            let mut descriptor = std::ptr::null_mut();

            let status = unsafe {
                GetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut acl,
                    std::ptr::null_mut(),
                    &mut descriptor,
                )
            };
            assert_eq!(
                status,
                ERROR_SUCCESS,
                "GetNamedSecurityInfoW: {}",
                io::Error::from_raw_os_error(status as i32)
            );

            let mut control: u16 = 0;
            let mut revision: u32 = 0;
            let ok =
                unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) };
            assert_ne!(ok, 0, "{}", io::Error::last_os_error());

            let count = unsafe { (*acl).AceCount };
            let entries = (0..count as u32)
                .map(|index| {
                    let mut ace = std::ptr::null_mut();
                    let ok = unsafe { GetAce(acl, index, &mut ace) };
                    assert_ne!(ok, 0, "{}", io::Error::last_os_error());

                    // Every ACE this crate can produce is an access-allowed
                    // ACE, whose SID starts at the `SidStart` field.
                    let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
                    let sid = unsafe { std::ptr::addr_of!((*allowed).SidStart) }.cast::<u8>();
                    let sid_len = unsafe { GetLengthSid(sid as PSID) } as usize;

                    AceEntry {
                        ace_type: unsafe { (*allowed).Header.AceType },
                        mask: unsafe { (*allowed).Mask },
                        sid_bytes: unsafe { std::slice::from_raw_parts(sid, sid_len) }.to_vec(),
                    }
                })
                .collect();

            unsafe { LocalFree(descriptor.cast()) };

            Self {
                is_protected: control & SE_DACL_PROTECTED != 0,
                entries,
            }
        }
    }

    /// Put a full-control entry for `sid` on `path`, alongside one for the
    /// current account so the test process can still read the result back. The
    /// `sid` entry is marked inheritable, which is what makes it reach files
    /// created underneath when `path` is a directory.
    fn grant_full_control(path: &Path, sid: PSID) {
        let owner = OwnerSid::current().expect("token user");
        let mut buffer = vec![
            0u32;
            (std::mem::size_of::<ACL>()
                + 2 * (std::mem::size_of::<ACCESS_ALLOWED_ACE>()
                    - std::mem::size_of::<u32>())
                + unsafe { GetLengthSid(owner.as_psid()) } as usize
                + unsafe { GetLengthSid(sid) } as usize)
                .div_ceil(std::mem::size_of::<u32>())
        ];
        let len = buffer.len() * std::mem::size_of::<u32>();
        let acl = buffer.as_mut_ptr().cast::<ACL>();

        unsafe {
            assert_ne!(InitializeAcl(acl, len as u32, ACL_REVISION), 0);
            assert_ne!(
                AddAccessAllowedAceEx(acl, ACL_REVISION, 0, FILE_ALL_ACCESS, owner.as_psid()),
                0
            );
            assert_ne!(
                AddAccessAllowedAceEx(
                    acl,
                    ACL_REVISION,
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                    FILE_ALL_ACCESS,
                    sid,
                ),
                0
            );
        }

        let wide = wide(path);
        let status = unsafe {
            SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        };
        assert_eq!(
            status,
            ERROR_SUCCESS,
            "SetNamedSecurityInfoW: {}",
            io::Error::from_raw_os_error(status as i32)
        );
    }

    fn well_known_sid(kind: i32) -> Vec<u8> {
        // SECURITY_MAX_SID_SIZE; the call reports the length actually used.
        let mut buffer = vec![0u8; 68];
        let mut len = buffer.len() as u32;
        let ok = unsafe {
            CreateWellKnownSid(
                kind,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut len,
            )
        };
        assert_ne!(ok, 0, "{}", io::Error::last_os_error());
        buffer.truncate(len as usize);
        buffer
    }

    fn sid_ptr(sid: &[u8]) -> PSID {
        sid.as_ptr() as PSID
    }

    fn sids_match(left: &[u8], right: PSID) -> bool {
        unsafe { EqualSid(sid_ptr(left), right) != 0 }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }
}
