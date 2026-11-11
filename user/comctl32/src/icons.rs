//! LoadIconMetric and LoadIconWithScaleDown (comctl32 6, ordinals 380 and
//! 381), as Microsoft documents them and Wine's comctl32 misc.c tests
//! record of Windows: an icon at the system's small or large icon size,
//! or at any size, made from the module's nearest image at least that
//! large and scaled down (rather than a smaller one scaled up). Without
//! a module, a standard icon (an IDI_ id) or an .ico file (a path).
//! Failures are HRESULTs: E_INVALIDARG for a missing name or out pointer
//! or another metric; HRESULT_FROM_WIN32(ERROR_RESOURCE_TYPE_NOT_FOUND)
//! when there is no such icon, as Windows 10 and 11 answer for a missing
//! resource and a missing file alike.

#[cfg_attr(
    target_arch = "x86",
    link(name = "user32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "user32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn LoadImageW(
        module: usize,
        name: *const u16,
        kind: u32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> usize;
    fn CreateIconFromResourceEx(
        bits: *const u8,
        size: u32,
        icon: i32,
        version: u32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> usize;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn FindResourceW(module: usize, name: usize, kind: usize) -> usize;
    fn LoadResource(module: usize, res: usize) -> *const u8;
    fn SizeofResource(module: usize, res: usize) -> u32;
    fn SetLastError(code: u32);
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_RESOURCE_TYPE_NOT_FOUND).
const E_RESOURCE_TYPE_NOT_FOUND: i32 = 0x8007_0715u32 as i32;
const ERROR_RESOURCE_TYPE_NOT_FOUND: u32 = 1813;
const LIM_SMALL: i32 = 0;
const LIM_LARGE: i32 = 1;
const SM_CXICON: i32 = 11;
const SM_CYICON: i32 = 12;
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;
const RT_ICON: usize = 3;
const RT_GROUP_ICON: usize = 14;
const IMAGE_ICON: u32 = 1;
const LR_LOADFROMFILE: u32 = 0x10;

/// The RT_ICON id of the group's image to scale down to `want`: the
/// smallest at least that wide, else the largest.
fn pick_down(module: usize, name: *const u16, want: i32) -> Option<u16> {
    // SAFETY: kernel32's resource functions on the caller's module; the
    // group's entries are read within its size.
    unsafe {
        let r = FindResourceW(module, name as usize, RT_GROUP_ICON);
        if r == 0 {
            return None;
        }
        let n = SizeofResource(module, r) as usize;
        let p = LoadResource(module, r);
        if p.is_null() || n < 6 {
            return None;
        }
        let d = core::slice::from_raw_parts(p, n);
        let count = u16::from_le_bytes([d[4], d[5]]) as usize;
        let mut above: Option<(i32, u16)> = None;
        let mut largest: Option<(i32, u16)> = None;
        for k in 0..count {
            let Some(e) = d.get(6 + 14 * k..20 + 14 * k) else { break };
            let side = if e[0] == 0 { 256 } else { i32::from(e[0]) };
            let id = u16::from_le_bytes([e[12], e[13]]);
            if side >= want && above.is_none_or(|(s, _)| side < s) {
                above = Some((side, id));
            }
            if largest.is_none_or(|(s, _)| side > s) {
                largest = Some((side, id));
            }
        }
        above.or(largest).map(|(_, id)| id)
    }
}

/// LoadIconWithScaleDown(hinst, pszName, cx, cy, phico).
#[unsafe(no_mangle)]
pub extern "system" fn LoadIconWithScaleDown(
    module: usize,
    name: *const u16,
    cx: i32,
    cy: i32,
    out: *mut usize,
) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = 0 };
    if name.is_null() {
        return E_INVALIDARG;
    }
    let icon = if module == 0 {
        // SAFETY: plain calls (a standard icon id, or a path).
        unsafe {
            if (name as usize) >> 16 == 0 {
                LoadImageW(0, name, IMAGE_ICON, cx, cy, 0)
            } else {
                LoadImageW(0, name, IMAGE_ICON, cx, cy, LR_LOADFROMFILE)
            }
        }
    } else {
        match pick_down(module, name, cx.max(cy)) {
            // SAFETY: the module's RT_ICON resource, read within its size.
            Some(id) => unsafe {
                let r = FindResourceW(module, usize::from(id), RT_ICON);
                let p = if r == 0 { core::ptr::null() } else { LoadResource(module, r) };
                if p.is_null() {
                    0
                } else {
                    CreateIconFromResourceEx(p, SizeofResource(module, r), 1, 0x30000, cx, cy, 0)
                }
            },
            None => 0,
        }
    };
    if icon == 0 {
        // SAFETY: a plain call.
        unsafe { SetLastError(ERROR_RESOURCE_TYPE_NOT_FOUND) };
        return E_RESOURCE_TYPE_NOT_FOUND;
    }
    // SAFETY: the caller's out pointer.
    unsafe { *out = icon };
    S_OK
}

/// LoadIconMetric(hinst, pszName, lims, phico): LIM_SMALL is
/// SM_CXSMICON x SM_CYSMICON, LIM_LARGE SM_CXICON x SM_CYICON.
#[unsafe(no_mangle)]
pub extern "system" fn LoadIconMetric(
    module: usize,
    name: *const u16,
    metric: i32,
    out: *mut usize,
) -> i32 {
    // SAFETY: plain calls.
    let (cx, cy) = unsafe {
        match metric {
            LIM_SMALL => (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)),
            LIM_LARGE => (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON)),
            _ => {
                if !out.is_null() {
                    *out = 0;
                }
                return E_INVALIDARG;
            }
        }
    };
    LoadIconWithScaleDown(module, name, cx, cy, out)
}
