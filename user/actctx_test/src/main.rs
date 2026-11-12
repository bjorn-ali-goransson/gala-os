//! Activation contexts with real manifests (ROADMAP OF: what Office's
//! programs ask of them), with the results of Windows. One source for two
//! programs: `actctx_test` (x64) and `wow64_actctx_test` (x86, built from
//! this file by user/wow64_actctx_test); both carry `manifest.xml` as
//! their RT_MANIFEST 1 (build.rs), the process's default context.
//!
//! Expected values come from Microsoft's documentation (CreateActCtx,
//! QueryActCtxW, FindActCtxSectionString/Guid, the manifest schema) and
//! from Wine's conformance tests run on Windows (dlls/kernel32/tests
//! actctx.c: test_actctx, test_detailed_info, test_info_in_assembly,
//! test_file_info, test_runlevel_info, test_find_dll_redirection,
//! test_find_window_class, test_find_com_redirection,
//! test_find_progid_redirection, test_wndclass_section,
//! test_dllredirect_section, test_compatibility, test_create_fail and
//! the manifests they use; THIRD_PARTY.md).
//!
//! - The process default (this program's manifest): GetCurrentActCtx
//!   NULL; QueryActCtxW of NULL: detailed information (format 1, one
//!   assembly, its manifest a file, the application directory the
//!   executable's), the assembly's encoded identity, run level asInvoker,
//!   the compatibility list (Windows 10's GUID, 40 bytes); its file
//!   (DLL redirection), window class (`1.2.3.4!GalaActCtxClass`), COM
//!   class and ProgID are found with nothing activated.
//! - Manifest files in %TEMP%: a context's detailed information (the
//!   manifest's full path, the application directory, configuration type
//!   NONE), its assemblies' information (encoded identities with the
//!   attributes in order; a dependency's directory name, the root's none;
//!   manifest version 1.0, no policy), files (`ulFlags` 2, the name, 0
//!   bytes written on success); dependencies found beside the manifest
//!   (count 2); DLL redirection data (20 bytes, PATH_OMITS_ASSEMBLY_ROOT,
//!   the dependency's roster index, the context with
//!   FIND_ACTCTX_SECTION_KEY_RETURN_HACTCTX); window classes (versioned
//!   by the assembly's version unless versioned="no", the module after
//!   the name, one section per context: four classes of two assemblies);
//!   COM classes (120-byte records, threading model, CLSID twice, type
//!   library, ProgID after the record, the misc-status mask; the alias
//!   GUID finds the same record; every ProgID points to the alias);
//!   run levels (asInvoker, requireAdministrator with uiAccess);
//!   compatibility lists in manifest order with maxversiontested
//!   (10.0.18358 is 0x000a000047b60000).
//! - Refused manifests (ERROR_SXS_CANT_GEN_ACTCTX): no namespace, no
//!   manifestVersion, an unknown attribute or element, content after the
//!   root, another namespace, a short file hash, a file without a name, a
//!   missing dependency or one of another version, UTF-16 without its
//!   mark; with the mark it is read. A missing file: ERROR_FILE_NOT_FOUND.
//!   An ACTCTXW that ends after lpSource is enough; one that ends before
//!   it is ERROR_INVALID_PARAMETER.
//!
//! `<name>: info ...` lines show what Windows answers where the tests
//! this follows do not say (not checked).
//!
//! Prints `<name>: ok` and exits with 0, or prints `<name>: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "actctx_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_actctx_test";
#[cfg(target_arch = "x86_64")]
const ARCH: &str = "amd64";
#[cfg(target_arch = "x86")]
const ARCH: &str = "x86";

type Handle = usize;
const INVALID: Handle = usize::MAX;

/// ACTCTXW.
#[repr(C)]
struct ActCtx {
    size: u32,
    flags: u32,
    source: *const u16,
    architecture: u16,
    language: u16,
    assembly_directory: *const u16,
    resource_name: *const u16,
    application_name: *const u16,
    module: Handle,
}

/// ACTCTX_SECTION_KEYED_DATA (winbase.h), with its assembly metadata.
#[repr(C)]
#[derive(Clone, Copy)]
struct KeyedData {
    size: u32,
    version: u32,
    data: usize,
    length: u32,
    global: usize,
    global_length: u32,
    base: usize,
    total_length: u32,
    actctx: Handle,
    roster: u32,
    flags: u32,
    metadata: [usize; 5],
}

impl KeyedData {
    /// Filled with 0xfe, as Wine's tests start, but for cbSize.
    fn new() -> KeyedData {
        // SAFETY: plain old data; every bit pattern is valid.
        let mut d: KeyedData = unsafe { core::mem::transmute([0xfeu8; size_of::<KeyedData>()]) };
        d.size = size_of::<KeyedData>() as u32;
        d
    }
}

/// ACTIVATION_CONTEXT_DETAILED_INFORMATION.
#[repr(C)]
#[derive(Default)]
struct Detailed {
    flags: u32,
    format_version: u32,
    assembly_count: u32,
    root_manifest_type: u32,
    root_manifest_chars: u32,
    root_config_type: u32,
    root_config_chars: u32,
    appdir_type: u32,
    appdir_chars: u32,
    root_manifest: usize,
    root_config: usize,
    appdir: usize,
}

/// ACTIVATION_CONTEXT_ASSEMBLY_DETAILED_INFORMATION.
#[repr(C)]
struct AssemblyInfo {
    flags: u32,
    identity_bytes: u32,
    manifest_type: u32,
    manifest_bytes: u32,
    manifest_time: u64,
    policy_type: u32,
    policy_bytes: u32,
    policy_time: u64,
    satellite: u32,
    manifest_major: u32,
    manifest_minor: u32,
    policy_major: u32,
    policy_minor: u32,
    directory_bytes: u32,
    identity: usize,
    manifest: usize,
    policy: usize,
    directory: usize,
    file_count: u32,
}

/// ASSEMBLY_FILE_DETAILED_INFORMATION.
#[repr(C)]
struct FileInfo {
    flags: u32,
    name_bytes: u32,
    path_bytes: u32,
    name: usize,
    path: usize,
}

const _: () = assert!(size_of::<Detailed>() == if cfg!(target_arch = "x86") { 48 } else { 64 });
const _: () =
    assert!(size_of::<AssemblyInfo>() == if cfg!(target_arch = "x86") { 88 } else { 104 });
const _: () = assert!(size_of::<KeyedData>() == if cfg!(target_arch = "x86") { 64 } else { 112 });

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn CreateDirectoryW(name: *const u16, attributes: usize) -> i32;
    fn RemoveDirectoryW(name: *const u16) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> Handle;
    fn DeleteFileW(name: *const u16) -> i32;
    fn GetModuleFileNameW(module: usize, buf: *mut u16, len: u32) -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;

    fn CreateActCtxW(ctx: *const ActCtx) -> Handle;
    fn ActivateActCtx(ctx: Handle, cookie: *mut usize) -> i32;
    fn DeactivateActCtx(flags: u32, cookie: usize) -> i32;
    fn GetCurrentActCtx(ctx: *mut Handle) -> i32;
    fn ReleaseActCtx(ctx: Handle);
    fn QueryActCtxW(
        flags: u32,
        ctx: Handle,
        sub: *const c_void,
        class: u32,
        buffer: *mut c_void,
        size: usize,
        written: *mut usize,
    ) -> i32;
    fn FindActCtxSectionStringW(
        flags: u32,
        guid: *const c_void,
        section: u32,
        name: *const u16,
        data: *mut KeyedData,
    ) -> i32;
    fn FindActCtxSectionStringA(
        flags: u32,
        guid: *const c_void,
        section: u32,
        name: *const u8,
        data: *mut KeyedData,
    ) -> i32;
    fn FindActCtxSectionGuid(
        flags: u32,
        guid: *const c_void,
        section: u32,
        key: *const [u8; 16],
        data: *mut KeyedData,
    ) -> i32;
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_SXS_CANT_GEN_ACTCTX: u32 = 14001;
const ERROR_SXS_SECTION_NOT_FOUND: u32 = 14004;
const ERROR_SXS_KEY_NOT_FOUND: u32 = 14007;

const DLL_REDIRECTION: u32 = 2;
const WINDOW_CLASS: u32 = 3;
const COM_SERVER: u32 = 4;
const COM_PROGID: u32 = 7;
const RETURN_HACTCTX: u32 = 1;
const QUERY_NO_ADDREF: u32 = 0x8000_0000;
const QUERY_USE_ACTIVE: u32 = 0x4;
const QUERY_IS_HMODULE: u32 = 0x8;
const PATH_NONE: u32 = 1;
const PATH_WIN32_FILE: u32 = 2;

// ---- Output and checks ---------------------------------------------------

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), text.as_ptr(), text.len() as u32, &mut written, 0)
    };
}

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn w(self, s: &[u16]) -> Self {
        let mut l = self;
        for &c in s {
            l = l.s(&[if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' }]);
        }
        l
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        print(&self.buf[..self.n]);
    }
}

/// An `info` line: what Windows answers, for the record.
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
}

fn info_text(what: &str, s: &[u16]) {
    Line::new().s(NAME.as_bytes()).s(b": info ").s(what.as_bytes()).s(b" ").w(s).s(b"\r\n").print();
}

fn section(what: &str) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what.as_bytes()).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call that failed (`ok` false) with `error`.
    #[track_caller]
    fn fails(&mut self, ok: bool, error: u32) -> Result<(), u32> {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        self.ok(!ok && e == error, u64::from(e))
    }

    /// Wide strings equal (case-insensitive when `ci`).
    #[track_caller]
    fn text(&mut self, got: &[u16], want: &[u16], ci: bool) -> Result<(), u32> {
        let same = if ci { eq_ci(got, want) } else { got == want };
        if !same {
            Line::new()
                .s(NAME.as_bytes())
                .s(b": got \"")
                .w(got)
                .s(b"\" want \"")
                .w(want)
                .s(b"\"\r\n")
                .print();
        }
        self.ok(same, got.len() as u64)
    }
}

fn eq_ci(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

/// A NUL-terminated UTF-16 string's units.
fn units<'a>(p: usize) -> &'a [u16] {
    let p = p as *const u16;
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    unsafe {
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// A NUL-terminated UTF-16 path or name, built up from pieces.
struct Wide {
    buf: [u16; 300],
    n: usize,
}

impl Wide {
    fn new() -> Wide {
        Wide { buf: [0; 300], n: 0 }
    }

    fn of(s: &str) -> Wide {
        Wide::new().a(s)
    }

    fn a(mut self, s: &str) -> Wide {
        for b in s.bytes() {
            self.buf[self.n] = u16::from(b);
            self.n += 1;
        }
        self.buf[self.n] = 0;
        self
    }

    fn u(mut self, s: &[u16]) -> Wide {
        self.buf[self.n..self.n + s.len()].copy_from_slice(s);
        self.n += s.len();
        self.buf[self.n] = 0;
        self
    }

    fn p(&self) -> *const u16 {
        self.buf.as_ptr()
    }

    fn s(&self) -> &[u16] {
        &self.buf[..self.n]
    }
}

/// An ASCII GUID string's 16 bytes (registry form without braces).
fn guid(s: &str) -> [u8; 16] {
    let b = s.as_bytes();
    let mut digits = [0u8; 32];
    let mut n = 0;
    for &c in b {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => continue,
        };
        digits[n] = d;
        n += 1;
    }
    let byte = |i: usize| digits[2 * i] << 4 | digits[2 * i + 1];
    let mut g = [0u8; 16];
    for (k, &i) in [3usize, 2, 1, 0, 5, 4, 7, 6].iter().enumerate() {
        g[k] = byte(i);
    }
    for (i, b) in g.iter_mut().enumerate().skip(8) {
        *b = byte(i);
    }
    g
}

fn rd32(p: usize) -> u32 {
    // SAFETY: inside a section the call returned.
    unsafe { (p as *const u32).read_unaligned() }
}

fn rd_guid(p: usize) -> [u8; 16] {
    // SAFETY: as above.
    unsafe { (p as *const [u8; 16]).read_unaligned() }
}

// ---- Manifests -----------------------------------------------------------

/// The directory of this run's files (%TEMP%\gala_<name>\).
fn work_dir() -> Wide {
    let mut t = [0u16; 260];
    // SAFETY: a buffer of its size.
    let n = unsafe { GetTempPathW(260, t.as_mut_ptr()) } as usize;
    Wide::new().u(&t[..n]).a("gala_").a(NAME).a("\\")
}

fn path(name: &str) -> Wide {
    work_dir().a(name)
}

/// Writes `text` to file `name` in the work directory.
fn put(name: &str, text: &[u8]) {
    let p = path(name);
    // SAFETY: valid arguments; GENERIC_WRITE, CREATE_ALWAYS.
    unsafe {
        let h = CreateFileW(p.p(), 0x4000_0000, 0, 0, 2, 0x80, 0);
        let mut written = 0;
        WriteFile(h, text.as_ptr(), text.len() as u32, &mut written, 0);
        CloseHandle(h);
    }
}

fn remove(name: &str) {
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path(name).p()) };
}

/// Up to 2 KiB of manifest text with "ARCH" replaced by this process's
/// architecture.
struct Text {
    buf: [u8; 2048],
    n: usize,
}

fn text(parts: &[&str]) -> Text {
    let mut t = Text { buf: [0; 2048], n: 0 };
    for p in parts {
        let b = p.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i..].starts_with(b"ARCH") {
                for &x in ARCH.as_bytes() {
                    t.buf[t.n] = x;
                    t.n += 1;
                }
                i += 4;
            } else {
                t.buf[t.n] = b[i];
                t.n += 1;
                i += 1;
            }
        }
    }
    t
}

impl Text {
    fn b(&self) -> &[u8] {
        &self.buf[..self.n]
    }
}

const HEAD: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">"#;

const MANIFEST1: &str = r#"<assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity></assembly>"#;

const MANIFEST2: &str = r#"<assemblyIdentity version="1.2.3.4" name="Wine.Test" type="win32"></assemblyIdentity><dependency><dependentAssembly><assemblyIdentity type="win32" name="testdep" version="6.5.4.3" processorArchitecture="ARCH"></assemblyIdentity></dependentAssembly></dependency></assembly>"#;

const DEP1: &str = r#"<assemblyIdentity type="win32" name="testdep" version="6.5.4.3" processorArchitecture="ARCH"/></assembly>"#;

const DEP2: &str = r#"<assemblyIdentity type="win32" name="testdep" version="6.5.4.3" processorArchitecture="ARCH" /><file name="testlib.dll"></file><file name="testlib2.dll" hash="63c978c2b53d6cf72b42fb7308f9af12ab19ec53" hashalg="SHA1" /></assembly>"#;

const DEP3: &str = r#"<assemblyIdentity type="win32" name="testdep" version="6.5.4.3" processorArchitecture="ARCH"/><file name="testlib.dll"/><file name="testlib2.dll" hash="63c978c2b53d6cf72b42fb7308f9af12ab19ec53" hashalg="SHA1"><windowClass>wndClass</windowClass><windowClass>wndClass2</windowClass></file></assembly>"#;

const WRONG_DEP1: &str = r#"<assemblyIdentity type="win32" name="testdep" version="6.5.4.4" processorArchitecture="ARCH" /></assembly>"#;

const MANIFEST3: &str = r#"<assemblyIdentity version="1.2.3.4"  name="Wine.Test" type="win32" publicKeyToken="6595b6414666f1df" /><description /><file name="testlib.dll"><windowClass>wndClass</windowClass><comClass description="Test com class" clsid="{12345678-1234-5678-1234-111122223333}" tlbid="{99999999-8888-7777-6666-555555555555}" threadingModel="Neutral" progid="ProgId.ProgId" miscStatus="cantlinkinside" miscStatusIcon="recomposeonresize" miscStatusContent="insideout" miscStatusThumbnail="alignable" miscStatusDocPrint="simpleframe,setclientsitefirst"><progid>ProgId.ProgId.1</progid><progid>ProgId.ProgId.2</progid></comClass><comClass clsid="{12345678-1234-5678-1234-111122223334}" threadingModel="Neutral" ><progid>ProgId.ProgId.7</progid></comClass></file></assembly>"#;

const WNDCLS1: &str = r#"<assemblyIdentity version="1.2.3.4"  name="testdep1" type="win32" processorArchitecture="ARCH"/><file name="testlib1.dll"><windowClass versioned="yes">wndClass1</windowClass><windowClass>wndClass2</windowClass> <typelib tlbid="{99999999-8888-7777-6666-555555555558}" version="1.0" helpdir="" /></file><file name="testlib1_2.dll" /></assembly>"#;

const WNDCLS2: &str = r#"<assemblyIdentity version="4.3.2.1"  name="testdep2" type="win32" processorArchitecture="ARCH" /><file name="testlib2.dll"> <windowClass versioned="no">wndClass3</windowClass> <windowClass>wndClass4</windowClass></file><file name="testlib2_2.dll" /></assembly>"#;

const WNDCLS_MAIN: &str = r#"<assemblyIdentity version="1.2.3.4" name="Wine.Test" type="win32" /><dependency> <dependentAssembly>  <assemblyIdentity type="win32" name="testdep1" version="1.2.3.4" processorArchitecture="ARCH" /> </dependentAssembly></dependency><dependency> <dependentAssembly>  <assemblyIdentity type="win32" name="testdep2" version="4.3.2.1" processorArchitecture="ARCH" /> </dependentAssembly></dependency></assembly>"#;

const MANIFEST5: &str = r#"<assemblyIdentity version="1.2.3.4" name="Wine.Test" type="win32"></assemblyIdentity><dependency>    <dependentAssembly dependencyType="preRequisite" allowDelayedBinding="true">        <assemblyIdentity name="Missing.Assembly" version="1.0.0.0" />    </dependentAssembly></dependency></assembly>"#;

const MANIFEST6: &str = r#"<assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity><trustInfo xmlns="urn:schemas-microsoft-com:asm.v1">    <security>        <requestedPrivileges>            <requestedExecutionLevel level="ASINVOKER" uiAccess="false"/>        </requestedPrivileges>    </security></trustInfo></assembly>"#;

const MANIFEST7: &str = r#"<assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity><trustInfo xmlns="urn:schemas-microsoft-com:asm.v2">    <security>        <requestedPrivileges>            <requestedExecutionLevel level="requireAdministrator" uiAccess="TRUE"/>        </requestedPrivileges>    </security></trustInfo></assembly>"#;

const COMPAT_NONE: &str = r#"   <assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity>   <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">       <application>       </application>   </compatibility></assembly>"#;

const COMPAT_MANY: &str = r#"   <assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity>   <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">       <application>           <supportedOS Id="{e2011457-1546-43c5-a5fe-008deee3d3f0}" ></supportedOS>           <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}" />           <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}" ></supportedOS>           <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />           <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}" />           <maxversiontested Id="10.0.18358" />           <maxversiontested Id="2.3.4.5" />       </application>   </compatibility></assembly>"#;

/// Manifests Windows refuses (Wine's wrong_manifest1-8).
const WRONG: [&str; 8] = [
    r#"<assembly manifestVersion="1.0"><assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity></assembly>"#,
    r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1"><assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity></assembly>"#,
    r#"<assembly test="test" xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity></assembly>"#,
    r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity><test></test></assembly>"#,
    r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity></assembly><test></test>"#,
    r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v5" manifestVersion="1.0"><assemblyIdentity version="1.0.0.0"  name="Wine.Test" type="win32"></assemblyIdentity></assembly>"#,
    r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity type="win32" name="testdep" version="6.5.4.3" processorArchitecture="ARCH" /><file name="testlib.dll" hash="63c978c2b53d6cf72b42fb7308f9af12ab19ec5" hashalg="SHA1" /></assembly>"#,
    r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><assemblyIdentity version="1.2.3.4"  name="Wine.Test" type="win32"></assemblyIdentity><file></file></assembly>"#,
];

/// CreateActCtxW of manifest file `name` in the work directory.
fn create(name: &str) -> Handle {
    let p = path(name);
    let ctx = ActCtx {
        size: size_of::<ActCtx>() as u32,
        flags: 0,
        source: p.p(),
        architecture: 0,
        language: 0,
        assembly_directory: null(),
        resource_name: null(),
        application_name: null(),
        module: 0,
    };
    // SAFETY: a valid ACTCTXW.
    unsafe { CreateActCtxW(&ctx) }
}

/// The executable's directory (with its backslash).
fn exe_dir() -> Wide {
    let mut b = [0u16; 260];
    // SAFETY: a buffer of its size.
    let n = unsafe { GetModuleFileNameW(0, b.as_mut_ptr(), 260) } as usize;
    let end = b[..n].iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    Wide::new().u(&b[..end])
}

// ---- Queries -------------------------------------------------------------

/// ActivationContextDetailedInformation of `h`: the too-small call's
/// size, then the full answer; checks the manifest path and appdir.
fn detailed(c: &mut Checks, h: Handle, manifest: Option<&[u16]>, count: u32) -> Result<(), u32> {
    let mut buf = [0usize; 256];
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        let mut small = Detailed::default();
        let r = QueryActCtxW(
            0,
            h,
            null(),
            2,
            (&raw mut small).cast(),
            size_of::<Detailed>(),
            &mut size,
        );
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        let appdir = exe_dir();
        let want =
            size_of::<Detailed>() + manifest.map_or(0, |m| 2 * (m.len() + 1)) + 2 * (appdir.n + 1);
        c.eq(size as u64, want as u64)?;
        let mut got = 0usize;
        c.ok(
            QueryActCtxW(0, h, null(), 2, buf.as_mut_ptr().cast(), size_of_val(&buf), &mut got)
                != 0,
            0,
        )?;
        c.eq(got as u64, want as u64)?;
        let d = &*buf.as_ptr().cast::<Detailed>();
        c.eq(d.flags.into(), 0)?;
        c.eq(d.format_version.into(), 1)?;
        c.eq(d.assembly_count.into(), count.into())?;
        c.eq(d.root_manifest_type.into(), PATH_WIN32_FILE.into())?;
        c.eq(d.root_config_type.into(), PATH_NONE.into())?;
        c.eq(d.root_config_chars.into(), 0)?;
        c.eq(d.root_config as u64, 0)?;
        c.eq(d.appdir_type.into(), PATH_WIN32_FILE.into())?;
        c.eq(d.appdir_chars.into(), appdir.n as u64)?;
        c.text(units(d.appdir), appdir.s(), true)?;
        if let Some(m) = manifest {
            c.eq(d.root_manifest_chars.into(), m.len() as u64)?;
            c.text(units(d.root_manifest), m, true)?;
        }
    }
    Ok(())
}

/// AssemblyDetailedInformationInActivationContext of assembly `id`:
/// its encoded identity, manifest path and whether it has a directory.
fn assembly(
    c: &mut Checks,
    h: Handle,
    id: u32,
    identity: &[u16],
    manifest: &[u16],
    has_directory: bool,
) -> Result<(), u32> {
    let mut buf = [0u64; 160];
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        let r = QueryActCtxW(
            0,
            h,
            (&raw const id).cast(),
            3,
            buf.as_mut_ptr().cast(),
            size_of::<AssemblyInfo>(),
            &mut size,
        );
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        let base = size_of::<AssemblyInfo>() + 2 * (identity.len() + 1) + 2 * (manifest.len() + 1);
        c.ok(size >= base, size as u64)?;
        let mut got = 0usize;
        c.ok(
            QueryActCtxW(
                0,
                h,
                (&raw const id).cast(),
                3,
                buf.as_mut_ptr().cast(),
                size_of_val(&buf),
                &mut got,
            ) != 0,
            0,
        )?;
        let a = &*buf.as_ptr().cast::<AssemblyInfo>();
        c.eq(
            got as u64,
            (base + if has_directory { a.directory_bytes as usize + 2 } else { 0 }) as u64,
        )?;
        c.eq(a.identity_bytes.into(), 2 * identity.len() as u64)?;
        c.text(units(a.identity), identity, false)?;
        c.eq(a.manifest_type.into(), PATH_WIN32_FILE.into())?;
        c.eq(a.manifest_bytes.into(), 2 * manifest.len() as u64)?;
        c.text(units(a.manifest), manifest, true)?;
        c.eq(a.policy_type.into(), PATH_NONE.into())?;
        c.eq(a.policy_bytes.into(), 0)?;
        c.eq(a.policy as u64, 0)?;
        c.eq(a.satellite.into(), 0)?;
        c.eq(a.manifest_major.into(), 1)?;
        c.eq(a.manifest_minor.into(), 0)?;
        c.eq(a.policy_major.into(), 0)?;
        c.eq(a.policy_minor.into(), 0)?;
        c.eq(u64::from(a.directory_bytes != 0), has_directory.into())?;
        c.eq(u64::from(a.directory != 0), has_directory.into())?;
        if id == 2 && has_directory {
            info_text("dependency's directory name", units(a.directory));
            info("its ulFlags", a.flags.into());
            info("its ulFileCount", a.file_count.into());
        }
    }
    Ok(())
}

/// FileInformationInAssemblyOfAssemblyInActivationContext of file
/// (`assembly`, `file`), both 0-based.
fn file(c: &mut Checks, h: Handle, assembly: u32, file: u32, name: &str) -> Result<(), u32> {
    let index = [assembly, file];
    let want = Wide::of(name);
    let mut buf = [0usize; 64];
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        let r = QueryActCtxW(
            0,
            h,
            index.as_ptr().cast(),
            4,
            buf.as_mut_ptr().cast(),
            size_of::<FileInfo>(),
            &mut size,
        );
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(size as u64, (size_of::<FileInfo>() + 2 * (want.n + 1)) as u64)?;
        c.ok(
            QueryActCtxW(
                0,
                h,
                index.as_ptr().cast(),
                4,
                buf.as_mut_ptr().cast(),
                size_of_val(&buf),
                &mut size,
            ) != 0,
            0,
        )?;
        c.eq(size as u64, 0)?;
        let f = &*buf.as_ptr().cast::<FileInfo>();
        c.eq(f.flags.into(), 2)?;
        c.eq(f.name_bytes.into(), 2 * want.n as u64)?;
        c.eq(f.path_bytes.into(), 0)?;
        c.text(units(f.name), want.s(), true)?;
        c.eq(f.path as u64, 0)?;
    }
    Ok(())
}

/// RunlevelInformationInActivationContext.
fn run_level(c: &mut Checks, h: Handle, level: u32, ui: u32) -> Result<(), u32> {
    let mut r = [0xffu32; 3];
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        c.ok(QueryActCtxW(0, h, null(), 5, r.as_mut_ptr().cast(), 12, &mut size) != 0, 0)?;
    }
    c.eq(size as u64, 12)?;
    c.eq(r[0].into(), 0)?;
    c.eq(r[1].into(), level.into())?;
    c.eq(r[2].into(), ui.into())
}

/// CompatibilityInformationInActivationContext: (count, elements).
fn compat(c: &mut Checks, h: Handle, buf: &mut [u64; 64]) -> Result<u32, u32> {
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        c.ok(
            QueryActCtxW(QUERY_NO_ADDREF, h, null(), 6, buf.as_mut_ptr().cast(), 512, &mut size)
                != 0,
            0,
        )?;
    }
    let count = buf[0] as u32;
    c.eq(size as u64, 8 + 32 * u64::from(count))?;
    Ok(count)
}

/// FindActCtxSectionStringW of DLL `name` in the active context(s):
/// the 20-byte record, its roster index; with RETURN_HACTCTX the context.
fn find_dll(c: &mut Checks, name: &str, roster: u32, h: Handle) -> Result<(), u32> {
    let n = Wide::of(name);
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionStringW(0, null(), DLL_REDIRECTION, n.p(), &mut d) != 0,
            GetLastError().into(),
        )?;
    }
    c.eq(d.size.into(), size_of::<KeyedData>() as u64)?;
    c.eq(d.version.into(), 1)?;
    c.eq(d.length.into(), 20)?;
    c.eq(rd32(d.data).into(), 20)?;
    c.eq(rd32(d.data + 4).into(), 2)?; // PATH_OMITS_ASSEMBLY_ROOT
    c.eq(rd32(d.data + 8).into(), 0)?;
    c.eq(rd32(d.data + 12).into(), 0)?;
    c.eq(rd32(d.data + 16).into(), 0)?;
    c.eq(d.global as u64, 0)?;
    c.eq(d.global_length.into(), 0)?;
    c.ok(d.base != 0 && d.total_length > 0, d.total_length.into())?;
    c.eq(d.actctx as u64, 0)?;
    c.eq(d.roster.into(), roster.into())?;
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionStringW(RETURN_HACTCTX, null(), DLL_REDIRECTION, n.p(), &mut d) != 0,
            0,
        )?;
    }
    c.eq(d.length.into(), 20)?;
    c.eq(d.actctx as u64, h as u64)?;
    // SAFETY: the reference the call took.
    unsafe { ReleaseActCtx(d.actctx) };
    Ok(())
}

/// FindActCtxSectionStringW of window class `name`: the record, its
/// versioned name `redirected`, the module after it.
fn find_class(
    c: &mut Checks,
    name: &str,
    redirected: &str,
    module: &str,
    roster: u32,
) -> Result<KeyedData, u32> {
    let n = Wide::of(name);
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionStringW(0, null(), WINDOW_CLASS, n.p(), &mut d) != 0,
            GetLastError().into(),
        )?;
    }
    c.eq(rd32(d.base).into(), 0x6448_7353)?; // "SsHd"
    c.ok(rd32(d.base + 20) > 0, 0)?;
    c.eq(d.version.into(), 1)?;
    let r = d.data;
    c.eq(rd32(r).into(), 24)?;
    c.eq(rd32(r + 4).into(), 0)?;
    let (name_len, name_offset, module_len, module_offset) =
        (rd32(r + 8), rd32(r + 12), rd32(r + 16), rd32(r + 20));
    c.eq(name_offset.into(), 24)?;
    let want = Wide::of(redirected);
    c.text(units(r + name_offset as usize), want.s(), false)?;
    c.eq(name_len.into(), 2 * want.n as u64)?;
    c.eq(d.length.into(), u64::from(24 + name_len + module_len + 4))?;
    // The module name follows the versioned name; module_offset is from
    // the section.
    c.eq((d.base + module_offset as usize) as u64, (r + 24 + name_len as usize + 2) as u64)?;
    c.text(units(d.base + module_offset as usize), Wide::of(module).s(), true)?;
    c.eq(d.global as u64, 0)?;
    c.eq(d.global_length.into(), 0)?;
    c.eq(d.actctx as u64, 0)?;
    c.eq(d.roster.into(), roster.into())?;
    Ok(d)
}

/// FindActCtxSectionGuid of COM class `clsid`: the 120-byte record.
fn find_com(
    c: &mut Checks,
    clsid: &[u8; 16],
    model: u32,
    tlbid: Option<&[u8; 16]>,
    progid: Option<&str>,
    roster: u32,
) -> Result<KeyedData, u32> {
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionGuid(0, null(), COM_SERVER, clsid, &mut d) != 0,
            GetLastError().into(),
        )?;
    }
    let r = d.data;
    c.eq(d.version.into(), 1)?;
    c.eq(rd32(r).into(), 120)?;
    c.eq(rd32(r + 8).into(), model.into())?;
    c.ok(rd_guid(r + 12) == *clsid, 0)?;
    c.ok(rd_guid(r + 44) == *clsid, 0)?;
    if let Some(t) = tlbid {
        c.ok(rd_guid(r + 60) == *t, 0)?;
    }
    c.ok(rd32(r + 76) > 0, 0)?; // the module name's length
    let (progid_len, progid_offset, clr_len) = (rd32(r + 84), rd32(r + 88), rd32(r + 92));
    c.eq(clr_len.into(), 0)?;
    match progid {
        Some(p) => {
            let want = Wide::of(p);
            c.eq(progid_offset.into(), 120)?;
            c.eq(progid_len.into(), 2 * want.n as u64)?;
            c.text(units(r + 120), want.s(), false)?;
            c.eq(d.length.into(), u64::from(120 + progid_len + 2))?;
        }
        None => {
            c.eq(progid_offset.into(), 0)?;
            c.eq(d.length.into(), 120)?;
        }
    }
    c.ok(d.total_length > rd32(r + 80), 0)?; // the module name is in the section
    c.eq((rd32(r + 4) & 0xffff_00ff).into(), 0)?;
    // The header's names area is the global data.
    c.eq(rd32(d.base).into(), 0x6448_7347)?; // "GsHd"
    c.eq(d.global as u64, (d.base + rd32(d.base + 32) as usize) as u64)?;
    c.eq(d.global_length.into(), rd32(d.base + 36).into())?;
    c.eq(d.actctx as u64, 0)?;
    c.eq(d.roster.into(), roster.into())?;
    // The alias finds the same record.
    let alias = rd_guid(r + 28);
    let mut d2 = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(FindActCtxSectionGuid(0, null(), COM_SERVER, &alias, &mut d2) != 0, 0)?;
    }
    c.eq(d2.data as u64, r as u64)?;
    Ok(d)
}

/// FindActCtxSectionStringA of ProgID `name`: it points to the alias of
/// `clsid`'s record.
fn find_progid(c: &mut Checks, name: &str, clsid: &[u8; 16], roster: u32) -> Result<(), u32> {
    let mut a = [0u8; 64];
    a[..name.len()].copy_from_slice(name.as_bytes());
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionStringA(0, null(), COM_PROGID, a.as_ptr(), &mut d) != 0,
            GetLastError().into(),
        )?;
    }
    let r = d.data;
    c.eq(d.version.into(), 1)?;
    c.eq(d.length.into(), 12)?;
    c.eq(rd32(r).into(), 12)?;
    c.eq(rd32(r + 4).into(), 0)?;
    let alias = rd_guid(d.base + rd32(r + 8) as usize);
    c.eq(d.global as u64, (d.base + rd32(d.base + 36) as usize) as u64)?;
    c.eq(d.global_length.into(), rd32(d.base + 40).into())?;
    c.eq(d.roster.into(), roster.into())?;
    let mut d2 = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(FindActCtxSectionGuid(0, null(), COM_SERVER, &alias, &mut d2) != 0, 0)?;
    }
    c.ok(rd_guid(d2.data + 28) == alias, 0)?;
    c.ok(rd_guid(d2.data + 12) == *clsid, 0)
}

// ---- The sections of the test --------------------------------------------

/// This program's own manifest: the process default context.
fn process_default(c: &mut Checks) -> Result<(), u32> {
    section("process default");
    let mut current: Handle = 1;
    // SAFETY: valid arguments.
    unsafe { c.ok(GetCurrentActCtx(&mut current) != 0, 0)? };
    c.eq(current as u64, 0)?;
    let mut buf = [0usize; 256];
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        c.ok(
            QueryActCtxW(
                QUERY_NO_ADDREF,
                0,
                null(),
                1,
                buf.as_mut_ptr().cast(),
                2 * size_of::<usize>(),
                &mut size,
            ) != 0,
            0,
        )?;
        c.eq(buf[0] as u64, 0)?;
        c.ok(
            QueryActCtxW(0, 0, null(), 2, buf.as_mut_ptr().cast(), size_of_val(&buf), &mut size)
                != 0,
            0,
        )?;
        let d = &*buf.as_ptr().cast::<Detailed>();
        c.eq(d.format_version.into(), 1)?;
        c.eq(d.assembly_count.into(), 1)?;
        info("process default's root manifest path type", d.root_manifest_type.into());
        info("process default's root manifest path characters", d.root_manifest_chars.into());
        if d.root_manifest != 0 {
            info_text("process default's root manifest path", units(d.root_manifest));
        }
        c.eq(d.appdir_type.into(), PATH_WIN32_FILE.into())?;
        c.text(units(d.appdir), exe_dir().s(), true)?;
    }
    let mut buf = [0u64; 160];
    let id = 1u32;
    // SAFETY: valid buffers.
    unsafe {
        c.ok(
            QueryActCtxW(
                0,
                0,
                (&raw const id).cast(),
                3,
                buf.as_mut_ptr().cast(),
                size_of_val(&buf),
                &mut size,
            ) != 0,
            0,
        )?;
        let a = &*buf.as_ptr().cast::<AssemblyInfo>();
        c.text(
            units(a.identity),
            Wide::of(r#"Gala.ActCtxTest,type="win32",version="1.2.3.4""#).s(),
            false,
        )?;
        info("process default's assembly manifest path type", a.manifest_type.into());
        c.eq(a.directory as u64, 0)?;
    }
    run_level(c, 0, 1, 0)?;
    let mut cbuf = [0u64; 64];
    let n = compat(c, 0, &mut cbuf)?;
    c.eq(n.into(), 1)?;
    // Element 0: Windows 10's GUID, ACTCTX_COMPATIBILITY_ELEMENT_TYPE_OS.
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = (cbuf[1 + i / 8] >> (8 * (i % 8))) as u8;
    }
    c.ok(g == guid("8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a"), 0)?;
    c.eq(cbuf[3] & 0xffff_ffff, 1)?;

    // Its sections, found with nothing activated.
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionStringW(
                0,
                null(),
                DLL_REDIRECTION,
                Wide::of("galaactctx.dll").p(),
                &mut d,
            ) != 0,
            GetLastError().into(),
        )?;
    }
    c.eq(d.length.into(), 20)?;
    c.eq(d.roster.into(), 1)?;
    c.eq(d.actctx as u64, 0)?;
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        let r = FindActCtxSectionStringW(
            RETURN_HACTCTX,
            null(),
            DLL_REDIRECTION,
            Wide::of("GALAACTCTX.DLL").p(),
            &mut d,
        );
        c.ok(r != 0, 0)?;
        info(
            "RETURN_HACTCTX's context for the process default is not NULL",
            u64::from(d.actctx != 0),
        );
        if d.actctx != 0 {
            ReleaseActCtx(d.actctx);
        }
    }
    find_class(c, "GalaActCtxClass", "1.2.3.4!GalaActCtxClass", "galaactctx.dll", 1)?;
    find_class(c, "galaactctxclass", "1.2.3.4!GalaActCtxClass", "galaactctx.dll", 1)?;
    let clsid = guid("6a4b2c10-1d2e-4f3a-9b8c-7d6e5f4a3b2c");
    find_com(c, &clsid, 1, None, Some("Gala.ActCtx.1"), 1)?;
    find_progid(c, "Gala.ActCtx.1", &clsid, 1)?;
    let mut d = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        let r = FindActCtxSectionStringW(
            0,
            null(),
            DLL_REDIRECTION,
            Wide::of("nothere.dll").p(),
            &mut d,
        );
        c.fails(r != 0, ERROR_SXS_KEY_NOT_FOUND)?;
        let r = FindActCtxSectionStringW(0, null(), 100, Wide::of("galaactctx.dll").p(), &mut d);
        c.fails(r != 0, ERROR_SXS_SECTION_NOT_FOUND)?;
        let r = FindActCtxSectionGuid(
            0,
            null(),
            COM_SERVER,
            &guid("6a4b2c10-1d2e-4f3a-9b8c-7d6e5f4a3b2d"),
            &mut d,
        );
        c.fails(r != 0, ERROR_SXS_KEY_NOT_FOUND)?;
        let r = FindActCtxSectionGuid(0, null(), COM_SERVER, &clsid, null_mut());
        c.fails(r != 0, ERROR_INVALID_PARAMETER)?;
    }
    // The executable's own module context, and the active one with
    // nothing active.
    // SAFETY: valid buffers.
    unsafe {
        let mut b = [7usize; 2];
        let r = QueryActCtxW(
            QUERY_IS_HMODULE | QUERY_NO_ADDREF,
            GetModuleHandleW(null()),
            null(),
            1,
            b.as_mut_ptr().cast(),
            size_of_val(&b),
            &mut size,
        );
        info("QUERY_ACTCTX_FLAG_ACTCTX_IS_HMODULE of the executable succeeds", r as u64);
        info("its context is not NULL", u64::from(b[0] != 0));
        let mut buf = [0usize; 256];
        let r = QueryActCtxW(
            QUERY_USE_ACTIVE,
            0,
            null(),
            2,
            buf.as_mut_ptr().cast(),
            size_of_val(&buf),
            &mut size,
        );
        info("USE_ACTIVE_ACTCTX detailed information with none active succeeds", r as u64);
        info("its assembly count", (*buf.as_ptr().cast::<Detailed>()).assembly_count.into());
    }
    Ok(())
}

/// Contexts of manifest files, Wine's test_actctx's.
fn manifests(c: &mut Checks) -> Result<(), u32> {
    section("manifest1");
    put("test1.manifest", text(&[HEAD, MANIFEST1]).b());
    let h = create("test1.manifest");
    c.ok(h != INVALID, 0)?;
    let m1 = path("test1.manifest");
    detailed(c, h, Some(m1.s()), 1)?;
    assembly(c, h, 1, Wide::of(r#"Wine.Test,type="win32",version="1.0.0.0""#).s(), m1.s(), false)?;
    let mut basic = [0usize; 2];
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        c.ok(
            QueryActCtxW(
                QUERY_NO_ADDREF,
                h,
                null(),
                1,
                basic.as_mut_ptr().cast(),
                size_of_val(&basic),
                &mut size,
            ) != 0,
            0,
        )?;
        c.eq(basic[0] as u64, h as u64)?;
        // USE_ACTIVE with a context: refused, the buffer left alone.
        let r = QueryActCtxW(
            QUERY_USE_ACTIVE | QUERY_NO_ADDREF,
            h,
            null(),
            1,
            basic.as_mut_ptr().cast(),
            size_of_val(&basic),
            &mut size,
        );
        c.fails(r != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(size as u64, 0)?;
        let id = 2u32;
        let mut b = [0u64; 64];
        let r = QueryActCtxW(
            0,
            h,
            (&raw const id).cast(),
            3,
            b.as_mut_ptr().cast(),
            size_of_val(&b),
            &mut size,
        );
        c.fails(r != 0, ERROR_INVALID_PARAMETER)?;
    }
    run_level(c, h, 0, 0)?;
    let mut cbuf = [0u64; 64];
    let n = compat(c, h, &mut cbuf)?;
    c.eq(n.into(), 0)?;
    // SAFETY: the context made above.
    unsafe { ReleaseActCtx(h) };
    remove("test1.manifest");

    section("manifest2 depmanifest1");
    put("test2.manifest", text(&[HEAD, MANIFEST2]).b());
    put("testdep.manifest", text(&[HEAD, DEP1]).b());
    let h = create("test2.manifest");
    c.ok(h != INVALID, 0)?;
    let m2 = path("test2.manifest");
    let dep = path("testdep.manifest");
    detailed(c, h, Some(m2.s()), 2)?;
    assembly(c, h, 1, Wide::of(r#"Wine.Test,type="win32",version="1.2.3.4""#).s(), m2.s(), false)?;
    let dep_id = Wide::of("testdep,processorArchitecture=\"")
        .a(ARCH)
        .a(r#"",type="win32",version="6.5.4.3""#);
    assembly(c, h, 2, dep_id.s(), dep.s(), true)?;
    // SAFETY: the context made above.
    unsafe { ReleaseActCtx(h) };

    section("manifest2 depmanifest2");
    put("testdep.manifest", text(&[HEAD, DEP2]).b());
    let h = create("test2.manifest");
    c.ok(h != INVALID, 0)?;
    assembly(c, h, 2, dep_id.s(), dep.s(), true)?;
    file(c, h, 1, 0, "testlib.dll")?;
    file(c, h, 1, 1, "testlib2.dll")?;
    let mut cookie = 0usize;
    // SAFETY: the context made above.
    unsafe { c.ok(ActivateActCtx(h, &mut cookie) != 0, 0)? };
    find_dll(c, "testlib.dll", 2, h)?;
    find_dll(c, "testlib2.dll", 2, h)?;
    let mut current = 0;
    // SAFETY: valid arguments.
    unsafe {
        c.ok(GetCurrentActCtx(&mut current) != 0, 0)?;
        c.eq(current as u64, h as u64)?;
        ReleaseActCtx(current);
        let mut d = KeyedData::new();
        let r = FindActCtxSectionStringW(
            0,
            null(),
            DLL_REDIRECTION,
            Wide::of("testlib3.dll").p(),
            &mut d,
        );
        c.fails(r != 0, ERROR_SXS_KEY_NOT_FOUND)?;
        c.ok(DeactivateActCtx(0, cookie) != 0, 0)?;
        ReleaseActCtx(h);
    }

    section("manifest2 depmanifest3");
    put("testdep.manifest", text(&[HEAD, DEP3]).b());
    let h = create("test2.manifest");
    c.ok(h != INVALID, 0)?;
    // SAFETY: the context made above.
    unsafe { c.ok(ActivateActCtx(h, &mut cookie) != 0, 0)? };
    let d = find_class(c, "wndClass", "6.5.4.3!wndClass", "testlib2.dll", 2)?;
    let d2 = find_class(c, "wndClass2", "6.5.4.3!wndClass2", "testlib2.dll", 2)?;
    c.eq(d.base as u64, d2.base as u64)?;
    c.eq(rd32(d.base + 20).into(), 2)?;
    // SAFETY: as above.
    unsafe {
        let mut d = KeyedData::new();
        c.ok(
            FindActCtxSectionStringW(
                RETURN_HACTCTX,
                null(),
                WINDOW_CLASS,
                Wide::of("wndClass").p(),
                &mut d,
            ) != 0,
            0,
        )?;
        c.eq(d.actctx as u64, h as u64)?;
        ReleaseActCtx(d.actctx);
        let r =
            FindActCtxSectionStringW(0, null(), WINDOW_CLASS, Wide::of("wndClass3").p(), &mut d);
        c.fails(r != 0, ERROR_SXS_KEY_NOT_FOUND)?;
        c.ok(DeactivateActCtx(0, cookie) != 0, 0)?;
        ReleaseActCtx(h);
    }

    section("manifest3");
    put("test3.manifest", text(&[HEAD, MANIFEST3]).b());
    let h = create("test3.manifest");
    c.ok(h != INVALID, 0)?;
    let m3 = path("test3.manifest");
    assembly(
        c,
        h,
        1,
        Wide::of(r#"Wine.Test,publicKeyToken="6595b6414666f1df",type="win32",version="1.2.3.4""#)
            .s(),
        m3.s(),
        false,
    )?;
    file(c, h, 0, 0, "testlib.dll")?;
    // SAFETY: the context made above.
    unsafe { c.ok(ActivateActCtx(h, &mut cookie) != 0, 0)? };
    find_dll(c, "testlib.dll", 1, h)?;
    let co = guid("12345678-1234-5678-1234-111122223333");
    let co2 = guid("12345678-1234-5678-1234-111122223334");
    let tlb = guid("99999999-8888-7777-6666-555555555555");
    let d = find_com(c, &co, 5, Some(&tlb), Some("ProgId.ProgId"), 1)?;
    // All five misc words, and their mask in flags' second byte.
    c.eq((rd32(d.data + 4) >> 8).into(), 0x1f)?;
    c.eq(rd32(d.data + 100).into(), 0x10)?; // cantlinkinside
    c.eq(rd32(d.data + 104).into(), 0x80)?; // insideout
    c.eq(rd32(d.data + 108).into(), 0x8000)?; // alignable
    c.eq(rd32(d.data + 112).into(), 0x1)?; // recomposeonresize
    c.eq(rd32(d.data + 116).into(), 0x30000)?; // simpleframe, setclientsitefirst
    let d2 = find_com(c, &co2, 5, None, None, 1)?;
    c.eq(rd32(d2.data + 4).into(), 0)?;
    for p in ["ProgId.ProgId", "ProgId.ProgId.1", "ProgId.ProgId.2"] {
        find_progid(c, p, &co, 1)?;
    }
    find_progid(c, "ProgId.ProgId.7", &co2, 1)?;
    // SAFETY: as above.
    unsafe {
        let mut d = KeyedData::new();
        let r = FindActCtxSectionStringA(
            0,
            null(),
            COM_PROGID,
            c"ProgId.ProgId.3".as_ptr().cast(),
            &mut d,
        );
        c.fails(r != 0, ERROR_SXS_KEY_NOT_FOUND)?;
        c.ok(DeactivateActCtx(0, cookie) != 0, 0)?;
        ReleaseActCtx(h);
    }
    remove("test3.manifest");

    section("wndcls sections");
    put("testdep1.manifest", text(&[HEAD, WNDCLS1]).b());
    put("testdep2.manifest", text(&[HEAD, WNDCLS2]).b());
    put("main_wndcls.manifest", text(&[HEAD, WNDCLS_MAIN]).b());
    let h = create("main_wndcls.manifest");
    c.ok(h != INVALID, 0)?;
    // SAFETY: the context made above.
    unsafe { c.ok(ActivateActCtx(h, &mut cookie) != 0, 0)? };
    let a = find_class(c, "wndClass1", "1.2.3.4!wndClass1", "testlib1.dll", 2)?;
    let b = find_class(c, "wndClass3", "wndClass3", "testlib2.dll", 3)?;
    c.eq(a.base as u64, b.base as u64)?;
    c.eq(a.total_length.into(), b.total_length.into())?;
    c.eq(rd32(a.base + 20).into(), 4)?;
    c.eq(rd32(a.base + 4).into(), 44)?;
    find_class(c, "wndClass4", "4.3.2.1!wndClass4", "testlib2.dll", 3)?;
    let mut d = KeyedData::new();
    let mut d2 = KeyedData::new();
    // SAFETY: valid arguments.
    unsafe {
        c.ok(
            FindActCtxSectionStringW(
                0,
                null(),
                DLL_REDIRECTION,
                Wide::of("testlib1.dll").p(),
                &mut d,
            ) != 0,
            0,
        )?;
        c.ok(
            FindActCtxSectionStringW(
                0,
                null(),
                DLL_REDIRECTION,
                Wide::of("testlib2.dll").p(),
                &mut d2,
            ) != 0,
            0,
        )?;
    }
    c.eq(rd32(d.base + 20).into(), 4)?;
    c.eq(rd32(d.base + 4).into(), 44)?;
    c.eq(d.base as u64, d2.base as u64)?;
    c.eq(d.roster.into(), 2)?;
    c.eq(d2.roster.into(), 3)?;
    let mut buf = [0usize; 256];
    let mut size = 0usize;
    // SAFETY: as above.
    unsafe {
        c.ok(
            QueryActCtxW(
                QUERY_USE_ACTIVE,
                0,
                null(),
                2,
                buf.as_mut_ptr().cast(),
                size_of_val(&buf),
                &mut size,
            ) != 0,
            0,
        )?;
        c.eq((*buf.as_ptr().cast::<Detailed>()).assembly_count.into(), 3)?;
        c.ok(DeactivateActCtx(0, cookie) != 0, 0)?;
        ReleaseActCtx(h);
    }
    for n in [
        "testdep1.manifest",
        "testdep2.manifest",
        "main_wndcls.manifest",
        "test2.manifest",
        "testdep.manifest",
    ] {
        remove(n);
    }
    Ok(())
}

/// Run levels and compatibility lists (Wine's test_runlevel_info and
/// test_compatibility).
fn levels_and_compat(c: &mut Checks) -> Result<(), u32> {
    section("run levels");
    for (m, level, ui) in [(MANIFEST6, 1, 0), (MANIFEST7, 3, 1)] {
        put("level.manifest", text(&[HEAD, m]).b());
        let h = create("level.manifest");
        c.ok(h != INVALID, 0)?;
        run_level(c, h, level, ui)?;
        // SAFETY: the context made above.
        unsafe { ReleaseActCtx(h) };
    }
    remove("level.manifest");

    section("compatibility");
    put("compat.manifest", text(&[HEAD, COMPAT_NONE]).b());
    let h = create("compat.manifest");
    c.ok(h != INVALID, 0)?;
    let mut cbuf = [0u64; 64];
    let n = compat(c, h, &mut cbuf)?;
    c.eq(n.into(), 0)?;
    // SAFETY: the context made above.
    unsafe { ReleaseActCtx(h) };
    put("compat.manifest", text(&[HEAD, COMPAT_MANY]).b());
    let h = create("compat.manifest");
    c.ok(h != INVALID, 0)?;
    let n = compat(c, h, &mut cbuf)?;
    c.eq(n.into(), 7)?;
    let ids = [
        "e2011457-1546-43c5-a5fe-008deee3d3f0",
        "35138b9a-5d96-4fbd-8e2d-a2440225f93a",
        "4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38",
        "8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a",
        "1f676c76-80e1-4239-95bb-83d0f6d0da78",
    ];
    // Elements of 32 bytes (4 u64s) after the 8-byte count.
    for (i, id) in ids.iter().enumerate() {
        let e = 1 + 4 * i;
        let mut g = [0u8; 16];
        for (k, b) in g.iter_mut().enumerate() {
            *b = (cbuf[e + k / 8] >> (8 * (k % 8))) as u8;
        }
        c.ok(g == guid(id), i as u64)?;
        c.eq(cbuf[e + 2] & 0xffff_ffff, 1)?;
    }
    for (i, v) in [0x000a_0000_47b6_0000u64, 0x0002_0003_0004_0005].iter().enumerate() {
        let e = 1 + 4 * (5 + i);
        c.eq(cbuf[e + 2] & 0xffff_ffff, 3)?;
        c.eq(cbuf[e + 3], *v)?;
    }
    // A buffer one element short: the size needed.
    let mut size = 0usize;
    // SAFETY: valid buffers.
    unsafe {
        let r = QueryActCtxW(
            QUERY_NO_ADDREF,
            h,
            null(),
            6,
            cbuf.as_mut_ptr().cast(),
            8 + 32 * 6,
            &mut size,
        );
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(size as u64, 8 + 32 * 7)?;
        ReleaseActCtx(h);
    }
    remove("compat.manifest");
    Ok(())
}

/// What CreateActCtxW refuses, and the smallest ACTCTXW it takes.
fn refused(c: &mut Checks) -> Result<(), u32> {
    section("refused manifests");
    for (i, m) in WRONG.iter().enumerate() {
        put("bad.manifest", text(&[m]).b());
        let h = create("bad.manifest");
        c.ok(h == INVALID, i as u64)?;
        c.fails(h != INVALID, ERROR_SXS_CANT_GEN_ACTCTX)?;
    }
    // A dependency missing, then one of another version.
    put("bad.manifest", text(&[HEAD, MANIFEST2]).b());
    remove("testdep.manifest");
    c.fails(create("bad.manifest") != INVALID, ERROR_SXS_CANT_GEN_ACTCTX)?;
    put("testdep.manifest", text(&[HEAD, WRONG_DEP1]).b());
    c.fails(create("bad.manifest") != INVALID, ERROR_SXS_CANT_GEN_ACTCTX)?;
    remove("testdep.manifest");
    // A missing assembly that allows delayed binding is no error.
    put("bad.manifest", text(&[HEAD, MANIFEST5]).b());
    let h = create("bad.manifest");
    c.ok(h != INVALID, 0)?;
    // SAFETY: the context made above.
    unsafe { ReleaseActCtx(h) };
    // UTF-16: without its byte-order mark refused, with it read.
    let t = text(&[HEAD, MANIFEST1]);
    let mut wide = [0u8; 600];
    for (i, &b) in t.b().iter().enumerate() {
        wide[2 + 2 * i] = b;
    }
    wide[0] = 0xff;
    wide[1] = 0xfe;
    put("bad.manifest", &wide[2..2 + 2 * t.n]);
    c.fails(create("bad.manifest") != INVALID, ERROR_SXS_CANT_GEN_ACTCTX)?;
    put("bad.manifest", &wide[..2 + 2 * t.n]);
    let h = create("bad.manifest");
    c.ok(h != INVALID, 0)?;
    // SAFETY: the context made above.
    unsafe { ReleaseActCtx(h) };
    remove("bad.manifest");
    c.fails(create("nonexistent.manifest") != INVALID, ERROR_FILE_NOT_FOUND)?;

    // cbSize: through lpSource is enough, less is refused.
    put("test1.manifest", text(&[HEAD, MANIFEST1]).b());
    let p = path("test1.manifest");
    let mut ctx = ActCtx {
        size: core::mem::offset_of!(ActCtx, architecture) as u32,
        flags: 0,
        source: p.p(),
        architecture: 0,
        language: 0,
        assembly_directory: null(),
        resource_name: null(),
        application_name: null(),
        module: 0,
    };
    // SAFETY: valid ACTCTXWs.
    unsafe {
        let h = CreateActCtxW(&ctx);
        c.ok(h != INVALID, GetLastError().into())?;
        ReleaseActCtx(h);
        ctx.size = core::mem::offset_of!(ActCtx, source) as u32;
        c.fails(CreateActCtxW(&ctx) != INVALID, ERROR_INVALID_PARAMETER)?;
        // ASSEMBLY_DIRECTORY_VALID with a relative name: in that directory.
        let dir = work_dir();
        let name = Wide::of("test1.manifest");
        let ctx = ActCtx {
            size: size_of::<ActCtx>() as u32,
            flags: 0x4,
            source: name.p(),
            architecture: 0,
            language: 0,
            assembly_directory: dir.p(),
            resource_name: null(),
            application_name: null(),
            module: 0,
        };
        let h = CreateActCtxW(&ctx);
        c.ok(h != INVALID, GetLastError().into())?;
        ReleaseActCtx(h);
    }
    remove("test1.manifest");
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let dir = work_dir();
    // SAFETY: a NUL-terminated path.
    unsafe { CreateDirectoryW(dir.p(), 0) };
    process_default(&mut c)?;
    manifests(&mut c)?;
    levels_and_compat(&mut c)?;
    refused(&mut c)?;
    // SAFETY: as above.
    unsafe { RemoveDirectoryW(dir.p()) };
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
