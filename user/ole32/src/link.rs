//! The standard link object: what a container gets from
//! OleCreateLink(Ex) for a source document it does not own, and the
//! object Word asks ole32 for after OleLoad (ROADMAP OF).
//!
//! Every answer here is measured on Windows 11 by `user/olelink_test`,
//! whose head comment carries the full contract. In outline:
//!
//! - The arguments are refused in one pass before anything is called:
//!   a NULL out pointer, moniker or storage; a renderopt outside 0..3;
//!   OLERENDER_FORMAT without a format; the format and advise-flag
//!   arrays required exactly when `cFormats` is non-zero and refused
//!   otherwise; an advise sink with anything but OLERENDER_FORMAT.
//!   OLERENDER_DRAW with a format is DV_E_CLIPFORMAT. A NULL riid is
//!   E_INVALIDARG too, but only after the bind -- the one argument
//!   check that costs the caller a call.
//! - The call then binds the source for IDataObject. Its failure is
//!   ignored for OLERENDER_NONE and fatal as OLE_E_CANT_BINDTOSOURCE
//!   for the other three, which need a presentation. The object is
//!   made and asked for the caller's interface (an interface it cannot
//!   make is E_NOINTERFACE with the storage never touched), the
//!   storage is marked with CLSID_StdOleLink (the one storage refusal
//!   that is fatal), Stat and OpenStream("\1Ole"-less `CompObj`) are
//!   made and their answers ignored, the client site is asked for the
//!   container's moniker, and the source is asked whether it is
//!   running. Nothing is written at create time.
//! - The object holds one reference on the moniker, one on the client
//!   site and two on the storage (its own and its cache's), and lets
//!   all of them go with itself. Nothing is cached between calls: two
//!   links on one storage are two objects.
//! - An unbound link leans on three errors: MK_E_UNAVAILABLE for
//!   anything that would have to bind, OLE_E_NOTRUNNING for anything
//!   that would have to ask the source for data, and
//!   REGDB_E_CLASSNOTREG for anything that would have to read the
//!   source's registration. The advise holders, the update option and
//!   the view advise are the link's own and work without a source.
//! - IPersistStorage::Save writes one stream, `\1Ole`, of 96 + L bytes
//!   around the L the source moniker's own Save writes, destroys
//!   `\2OlePres000` after it, and asks the client site for the
//!   container's moniker three times on the way. The layout is the
//!   OLEStream of MS-OLEDS; `write_head` below names each field.
//!
//! What gala does not have is the cached presentation: a link that
//! binds its source, draws, or holds a format in its cache is past
//! this module, and every such call records its own row rather than
//! answering with a lie (OBSERVABILITY; ARCHITECTURE "COM: ole32 and
//! oleaut32").
//!
//! Where a method's answer could not be measured because Windows
//! faults on it (a NULL out pointer on the getters), gala refuses it
//! with E_POINTER, as the moniker module does.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN,
    S_FALSE, S_OK, VTable, add_ref, alloc_zeroed, com_call, free, missing, ole_iid, put, release,
};

const E_FAIL: HResult = 0x8000_4005u32 as i32;
const DV_E_CLIPFORMAT: HResult = 0x8004_006au32 as i32;
const OLE_E_CANT_BINDTOSOURCE: HResult = 0x8004_000au32 as i32;
const OLE_E_NOCONNECTION: HResult = 0x8004_0004u32 as i32;
const OLE_E_NOTRUNNING: HResult = 0x8004_0005u32 as i32;
const OLE_E_BLANK: HResult = 0x8004_0007u32 as i32;
const REGDB_E_CLASSNOTREG: HResult = 0x8004_0154u32 as i32;
const MK_E_UNAVAILABLE: HResult = 0x8004_01e3u32 as i32;
const CO_E_ALREADYINITIALIZED: HResult = 0x8004_01f1u32 as i32;

const IID_IPERSIST: Guid = ole_iid(0x10c);
const IID_IPERSISTSTORAGE: Guid = ole_iid(0x10a);
const IID_IENUMSTATDATA: Guid = ole_iid(0x105);
const IID_IDATAOBJECT: Guid = ole_iid(0x10e);
const IID_IVIEWOBJECT: Guid = ole_iid(0x10d);
const IID_IVIEWOBJECT2: Guid = ole_iid(0x127);
const IID_IOLEOBJECT: Guid = ole_iid(0x112);
const IID_IOLELINK: Guid = ole_iid(0x11d);
const IID_IOLECACHE: Guid = ole_iid(0x11e);
const IID_IOLECACHE2: Guid = ole_iid(0x128);
const IID_IRUNNABLEOBJECT: Guid = ole_iid(0x126);

/// CLSID_StdOleLink {00000300-0000-0000-C000-000000000046}: the class
/// the storage is marked with, which the object also reports as its
/// own and as its running class.
const CLSID_STDOLELINK: Guid = ole_iid(0x300);
const GUID_NULL: Guid = [0; 16];

/// The slots this module calls on the caller's objects.
const MONIKER_GET_CLASS_ID: usize = 3;
const MONIKER_SAVE: usize = 6;
const MONIKER_BIND_TO_OBJECT: usize = 8;
const MONIKER_IS_RUNNING: usize = 15;
const MONIKER_GET_DISPLAY_NAME: usize = 20;
const STORAGE_CREATE_STREAM: usize = 3;
const STORAGE_OPEN_STREAM: usize = 4;
const STORAGE_DESTROY_ELEMENT: usize = 12;
const STORAGE_SET_CLASS: usize = 15;
const STORAGE_STAT: usize = 17;
const STREAM_WRITE: usize = 4;
const STREAM_SEEK: usize = 5;
const CLIENTSITE_GET_MONIKER: usize = 4;

/// A STATSTG is 80 bytes on x64 (72 on x86, which fits), and the Stat
/// at create time is made with STATFLAG_NONAME so nothing is allocated
/// for a name that is not read.
const STATSTG_BYTES: usize = 80;
const STATFLAG_NONAME: u32 = 1;

const STGM_READWRITE: u32 = 2;
const STGM_SHARE_EXCLUSIVE: u32 = 0x10;
const STGM_CREATE: u32 = 0x1000;

const STREAM_SEEK_SET: u32 = 0;
const STREAM_SEEK_CUR: u32 = 1;

const OLERENDER_NONE: u32 = 0;
const OLERENDER_DRAW: u32 = 1;
const OLERENDER_FORMAT: u32 = 2;
const OLERENDER_ASIS: u32 = 3;

const OLEUPDATE_ALWAYS: u32 = 1;
const OLEUPDATE_ONCALL: u32 = 3;

const OLEGETMONIKER_ONLYIFTHERE: u32 = 1;
const OLEWHICHMK_CONTAINER: u32 = 1;

/// The status bits an unbound link reports along with
/// REGDB_E_CLASSNOTREG: OLEMISC_CANLINKBYOLE1 | OLEMISC_CANTLINKINSIDE
/// | OLEMISC_RECOMPOSEONRESIZE's neighbours -- 0x50 as measured, which
/// is what a link whose source is not registered answers.
const LINK_MISC_STATUS: u32 = 0x50;

/// `\1CompObj`, `\1Ole` and `\2OlePres000`: the three streams the link
/// names, spelled out so no escape hides the leading control
/// character.
const COMPOBJ_STREAM: [u16; 9] = [
    1,
    b'C' as u16,
    b'o' as u16,
    b'm' as u16,
    b'p' as u16,
    b'O' as u16,
    b'b' as u16,
    b'j' as u16,
    0,
];
const OLE_STREAM: [u16; 5] = [1, b'O' as u16, b'l' as u16, b'e' as u16, 0];
const OLEPRES_STREAM: [u16; 12] = [
    2,
    b'O' as u16,
    b'l' as u16,
    b'e' as u16,
    b'P' as u16,
    b'r' as u16,
    b'e' as u16,
    b's' as u16,
    b'0' as u16,
    b'0' as u16,
    b'0' as u16,
    0,
];

/// How many sinks one holder keeps. Windows has no such limit; one
/// past this is E_OUTOFMEMORY with a row naming the holder.
const SINKS: usize = 8;

/// One advise holder: the link's own, which works without a source.
/// `made` is what IOleObject::EnumAdvise answers on -- E_FAIL before
/// the first Advise has made the holder, E_NOTIMPL once it has, as
/// measured.
#[repr(C)]
struct Holder {
    sinks: [*mut c_void; SINKS],
    conns: [u32; SINKS],
    next: u32,
    made: u32,
}

fn empty_holder() -> Holder {
    Holder { sinks: [core::ptr::null_mut(); SINKS], conns: [0; SINKS], next: 0, made: 0 }
}

impl Holder {
    /// Takes `sink` and hands back its connection, or 0 when the
    /// holder is full.
    fn add(&mut self, sink: *mut c_void) -> u32 {
        self.made = 1;
        for i in 0..SINKS {
            if self.conns[i] == 0 {
                self.next += 1;
                self.conns[i] = self.next;
                self.sinks[i] = sink;
                if !sink.is_null() {
                    add_ref(sink);
                }
                return self.next;
            }
        }
        0
    }

    fn remove(&mut self, conn: u32) -> bool {
        for i in 0..SINKS {
            if self.conns[i] == conn && conn != 0 {
                self.conns[i] = 0;
                let sink = core::mem::replace(&mut self.sinks[i], core::ptr::null_mut());
                if !sink.is_null() {
                    release(sink);
                }
                return true;
            }
        }
        false
    }

    fn held(&self) -> usize {
        self.conns.iter().filter(|c| **c != 0).count()
    }

    fn drop_all(&mut self) {
        for i in 0..SINKS {
            self.conns[i] = 0;
            let sink = core::mem::replace(&mut self.sinks[i], core::ptr::null_mut());
            if !sink.is_null() {
                release(sink);
            }
        }
    }
}

/// The link. Its head is one vtable pointer per interface it answers,
/// in the order `F_*` below name them: a secondary interface pointer is
/// the address of its own field, which `owner` turns back into the
/// object. IOleObject's is the first, so it is the object's identity
/// (IUnknown) as well.
#[repr(C)]
struct Link {
    oleobject_vtable: *const usize,
    olelink_vtable: *const usize,
    persist_vtable: *const usize,
    data_vtable: *const usize,
    view_vtable: *const usize,
    cache_vtable: *const usize,
    runnable_vtable: *const usize,
    refs: AtomicU32,
    /// The source, the container's site and the storage the link was
    /// made on. The storage is held twice, as Windows holds it.
    moniker: *mut c_void,
    site: *mut c_void,
    storage: *mut c_void,
    update: u32,
    dirty: u32,
    /// IOleObject::Advise, IDataObject::DAdvise and
    /// IOleCache::Cache each keep their own connections.
    advise: Holder,
    data_advise: Holder,
    cache: Holder,
    view_sink: *mut c_void,
    view_aspects: u32,
    view_advf: u32,
}

const F_OLEOBJECT: usize = 0;
const F_OLELINK: usize = 1;
const F_PERSIST: usize = 2;
const F_DATA: usize = 3;
const F_VIEW: usize = 4;
const F_CACHE: usize = 5;
const F_RUNNABLE: usize = 6;

// The seven vtable fields are the object's head, one pointer each, so
// `owner` can subtract field * pointer size. If anything is ever put
// in front of them this stops compiling rather than handing out
// pointers into the middle of the object.
const _: () = assert!(
    core::mem::offset_of!(Link, oleobject_vtable)
        == F_OLEOBJECT * core::mem::size_of::<*const usize>()
);
const _: () = assert!(
    core::mem::offset_of!(Link, runnable_vtable)
        == F_RUNNABLE * core::mem::size_of::<*const usize>()
);

fn lk<'a>(this: *mut Link) -> &'a mut Link {
    // SAFETY: our object, alive while the caller holds a reference.
    unsafe { &mut *this }
}

/// The link behind a secondary interface pointer, which is the address
/// of vtable field `field`.
fn owner(this: *mut c_void, field: usize) -> *mut Link {
    // SAFETY: the address of field `field` of one of our objects.
    unsafe { this.cast::<u8>().sub(field * core::mem::size_of::<*const usize>()).cast::<Link>() }
}

/// A bind context for a call that needs one (the source moniker's
/// BindToObject, IsRunning and GetDisplayName all take one, and gala's
/// own monikers refuse a NULL).
fn bind_context() -> *mut c_void {
    let mut bc: *mut c_void = core::ptr::null_mut();
    if super::bindctx::CreateBindCtx(0, &mut bc) < 0 {
        return core::ptr::null_mut();
    }
    bc
}

/// Binds the source for a method that cannot work without it. An
/// unbound source answers with the moniker's own error, which is what
/// Windows passes on; a source that does bind is past what this module
/// can do with the object, so the row names the method and the object
/// goes back.
fn bind_source(l: &mut Link, what: &[u8]) -> HResult {
    if l.moniker.is_null() {
        return MK_E_UNAVAILABLE;
    }
    let bc = bind_context();
    if bc.is_null() {
        return E_OUTOFMEMORY;
    }
    let mut object: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        l.moniker,
        MONIKER_BIND_TO_OBJECT,
        fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HResult,
        bc,
        core::ptr::null_mut(),
        &IID_IOLEOBJECT,
        &mut object
    );
    release(bc);
    if hr < 0 {
        return hr;
    }
    if !object.is_null() {
        release(object);
    }
    missing(what, 0);
    E_NOTIMPL
}

// ---- IUnknown, shared by every interface ----

extern "system" fn query(this: *mut Link, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    let l = lk(this);
    // Each interface is handed out as the address of its own vtable
    // field; IOleObject's is the object itself, and so is IUnknown's.
    let field = if iid == IID_IUNKNOWN || iid == IID_IOLEOBJECT {
        &raw mut l.oleobject_vtable
    } else if iid == IID_IOLELINK {
        &raw mut l.olelink_vtable
    } else if iid == IID_IPERSISTSTORAGE || iid == IID_IPERSIST {
        // IPersist is IPersistStorage's own first four slots.
        &raw mut l.persist_vtable
    } else if iid == IID_IDATAOBJECT {
        &raw mut l.data_vtable
    } else if iid == IID_IVIEWOBJECT || iid == IID_IVIEWOBJECT2 {
        // As IViewObject is IViewObject2's first nine.
        &raw mut l.view_vtable
    } else if iid == IID_IOLECACHE || iid == IID_IOLECACHE2 {
        &raw mut l.cache_vtable
    } else if iid == IID_IRUNNABLEOBJECT {
        &raw mut l.runnable_vtable
    } else {
        // Measured refusals: IPersistStream, IMarshal, IOleWindow,
        // IExternalConnection, IStdMarshalInfo, IOleCacheControl and
        // IClassFactory are all E_NOINTERFACE with the caller's
        // pointer zeroed, and a program that asks for one must get
        // that rather than a vtable of the wrong shape.
        put(out, core::ptr::null_mut());
        return E_NOINTERFACE;
    };
    add(this);
    put(out, field.cast());
    S_OK
}

extern "system" fn add(this: *mut Link) -> u32 {
    lk(this).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn rel(this: *mut Link) -> u32 {
    let left = lk(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        let l = lk(this);
        l.advise.drop_all();
        l.data_advise.drop_all();
        l.cache.drop_all();
        if !l.view_sink.is_null() {
            release(l.view_sink);
        }
        if !l.moniker.is_null() {
            release(l.moniker);
        }
        if !l.site.is_null() {
            release(l.site);
        }
        if !l.storage.is_null() {
            // Both of them: the link's own and its cache's.
            release(l.storage);
            release(l.storage);
        }
        free(this.cast());
    }
    left
}

/// The three IUnknown slots of a secondary interface, which forward to
/// the object's own.
macro_rules! secondary {
    ($query:ident, $add:ident, $rel:ident, $field:expr) => {
        extern "system" fn $query(
            this: *mut c_void,
            iid: *const Guid,
            out: *mut *mut c_void,
        ) -> HResult {
            query(owner(this, $field), iid, out)
        }
        extern "system" fn $add(this: *mut c_void) -> u32 {
            add(owner(this, $field))
        }
        extern "system" fn $rel(this: *mut c_void) -> u32 {
            rel(owner(this, $field))
        }
    };
}

secondary!(link_query, link_add, link_rel, F_OLELINK);
secondary!(persist_query, persist_add, persist_rel, F_PERSIST);
secondary!(data_query, data_add, data_rel, F_DATA);
secondary!(view_query, view_add, view_rel, F_VIEW);
secondary!(cache_query, cache_add, cache_rel, F_CACHE);
secondary!(run_query, run_add, run_rel, F_RUNNABLE);

// ---- IOleObject ----

extern "system" fn set_client_site(this: *mut Link, site: *mut c_void) -> HResult {
    let l = lk(this);
    if !l.site.is_null() {
        release(l.site);
    }
    l.site = site;
    if !site.is_null() {
        add_ref(site);
    }
    S_OK
}

extern "system" fn get_client_site(this: *mut Link, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let l = lk(this);
    if !l.site.is_null() {
        add_ref(l.site);
    }
    put(out, l.site);
    S_OK
}

/// The container's names are taken and forgotten: an unbound link has
/// no server to pass them to.
extern "system" fn set_host_names(
    _: *mut Link,
    _container: *const u16,
    _object: *const u16,
) -> HResult {
    S_OK
}

/// Closing an unbound link is nothing to do, and may be done twice.
extern "system" fn close(_: *mut Link, _save: u32) -> HResult {
    S_OK
}

extern "system" fn set_moniker(_: *mut Link, _which: u32, _moniker: *mut c_void) -> HResult {
    S_OK
}

/// The container's moniker is the client site's business, and its
/// answer is passed on as it stands (a site that has none answers
/// E_NOTIMPL, which is no trouble). Without a site there is no one to
/// ask: E_FAIL, which Windows was not measured on.
extern "system" fn get_moniker(
    this: *mut Link,
    assign: u32,
    which: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let l = lk(this);
    if l.site.is_null() {
        return E_FAIL;
    }
    com_call!(
        l.site,
        CLIENTSITE_GET_MONIKER,
        fn(u32, u32, *mut *mut c_void) -> HResult,
        assign,
        which,
        out
    )
}

extern "system" fn init_from_data(
    _: *mut Link,
    _data: *mut c_void,
    _creation: i32,
    _reserved: u32,
) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn get_clipboard_data(
    _: *mut Link,
    _reserved: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    OLE_E_NOTRUNNING
}

#[allow(clippy::too_many_arguments)]
extern "system" fn do_verb(
    this: *mut Link,
    _verb: i32,
    _message: *mut c_void,
    _active_site: *mut c_void,
    _index: i32,
    _parent: *mut c_void,
    _position: *const c_void,
) -> HResult {
    bind_source(lk(this), b"ole32.dll!IOleObject::DoVerb(the source bound)")
}

extern "system" fn enum_verbs(_: *mut Link, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // The verbs live in the source's registration, which is not there.
    put(out, core::ptr::null_mut());
    REGDB_E_CLASSNOTREG
}

extern "system" fn update(this: *mut Link) -> HResult {
    bind_source(lk(this), b"ole32.dll!IOleObject::Update(the source bound)")
}

extern "system" fn is_up_to_date(this: *mut Link) -> HResult {
    bind_source(lk(this), b"ole32.dll!IOleObject::IsUpToDate(the source bound)")
}

/// An unbound link has no class of its own to report, where its
/// running class (IRunnableObject) is CLSID_StdOleLink.
extern "system" fn get_user_class_id(_: *mut Link, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, GUID_NULL);
    S_OK
}

extern "system" fn get_user_type(_: *mut Link, _form: u32, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    REGDB_E_CLASSNOTREG
}

extern "system" fn set_extent(_: *mut Link, _aspect: u32, _size: *const [i32; 2]) -> HResult {
    E_FAIL
}

/// Blank, and the caller's size is zeroed -- where IViewObject2's own
/// GetExtent leaves it alone. Both were measured; the difference is
/// not an accident of this module.
extern "system" fn get_extent(_: *mut Link, _aspect: u32, size: *mut [i32; 2]) -> HResult {
    if size.is_null() {
        return E_POINTER;
    }
    put(size, [0, 0]);
    OLE_E_BLANK
}

extern "system" fn advise(this: *mut Link, sink: *mut c_void, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let conn = lk(this).advise.add(sink);
    if conn == 0 {
        missing(b"ole32.dll!IOleObject::Advise(more sinks than the holder keeps)", SINKS as u64);
        return E_OUTOFMEMORY;
    }
    put(out, conn);
    S_OK
}

extern "system" fn unadvise(this: *mut Link, conn: u32) -> HResult {
    if lk(this).advise.remove(conn) { S_OK } else { OLE_E_NOCONNECTION }
}

/// Before the first Advise there is no holder: E_FAIL, with the
/// caller's pointer left as it was. Once there is one it keeps its
/// sinks but will not list them: E_NOTIMPL, with the pointer zeroed.
/// Both halves are measured, the pointer included.
extern "system" fn enum_advise(this: *mut Link, out: *mut *mut c_void) -> HResult {
    if lk(this).advise.made == 0 {
        return E_FAIL;
    }
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    E_NOTIMPL
}

/// The status bits live in the source's registration: the error comes
/// back and the bits are written anyway, which is what a container
/// that ignores the HRESULT reads.
extern "system" fn get_misc_status(_: *mut Link, _aspect: u32, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, LINK_MISC_STATUS);
    REGDB_E_CLASSNOTREG
}

extern "system" fn set_color_scheme(_: *mut Link, _palette: *const c_void) -> HResult {
    S_OK
}

static OLEOBJECT_VTABLE: VTable<24> = VTable([
    query as *const (),
    add as *const (),
    rel as *const (),
    set_client_site as *const (),
    get_client_site as *const (),
    set_host_names as *const (),
    close as *const (),
    set_moniker as *const (),
    get_moniker as *const (),
    init_from_data as *const (),
    get_clipboard_data as *const (),
    do_verb as *const (),
    enum_verbs as *const (),
    update as *const (),
    is_up_to_date as *const (),
    get_user_class_id as *const (),
    get_user_type as *const (),
    set_extent as *const (),
    get_extent as *const (),
    advise as *const (),
    unadvise as *const (),
    enum_advise as *const (),
    get_misc_status as *const (),
    set_color_scheme as *const (),
]);

// ---- IOleLink ----

/// OLEUPDATE_ALWAYS and OLEUPDATE_ONCALL are the two the header names
/// and the two the link takes; anything else is E_INVALIDARG and
/// changes nothing (measured).
extern "system" fn set_update_options(this: *mut c_void, option: u32) -> HResult {
    if option != OLEUPDATE_ALWAYS && option != OLEUPDATE_ONCALL {
        return E_INVALIDARG;
    }
    let l = lk(owner(this, F_OLELINK));
    l.update = option;
    l.dirty = 1;
    S_OK
}

extern "system" fn get_update_options(this: *mut c_void, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, lk(owner(this, F_OLELINK)).update);
    S_OK
}

/// A link takes a new source, and -- oddly, but measured -- takes none
/// at all without complaint.
extern "system" fn set_source_moniker(
    this: *mut c_void,
    moniker: *mut c_void,
    _clsid: *const Guid,
) -> HResult {
    let l = lk(owner(this, F_OLELINK));
    if !l.moniker.is_null() {
        release(l.moniker);
    }
    l.moniker = moniker;
    if !moniker.is_null() {
        add_ref(moniker);
    }
    l.dirty = 1;
    S_OK
}

extern "system" fn get_source_moniker(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let l = lk(owner(this, F_OLELINK));
    if l.moniker.is_null() {
        // A link whose source was taken away with SetSourceMoniker(NULL).
        return MK_E_UNAVAILABLE;
    }
    add_ref(l.moniker);
    put(out, l.moniker);
    S_OK
}

/// The name is parsed where every display name is parsed, so a file
/// that is not there is the moniker layer's own refusal
/// (MK_E_CANTOPENFILE, measured).
extern "system" fn set_source_display_name(this: *mut c_void, name: *const u16) -> HResult {
    if name.is_null() {
        return E_INVALIDARG;
    }
    let bc = bind_context();
    if bc.is_null() {
        return E_OUTOFMEMORY;
    }
    let mut eaten = 0u32;
    let mut moniker: *mut c_void = core::ptr::null_mut();
    let hr = super::moniker::MkParseDisplayName(bc, name, &mut eaten, &mut moniker);
    release(bc);
    if hr < 0 {
        if !moniker.is_null() {
            release(moniker);
        }
        return hr;
    }
    let l = lk(owner(this, F_OLELINK));
    if !l.moniker.is_null() {
        release(l.moniker);
    }
    l.moniker = moniker;
    l.dirty = 1;
    S_OK
}

/// The display name is the source moniker's, fetched when asked rather
/// than kept (measured: one GetDisplayName per call).
extern "system" fn get_source_display_name(this: *mut c_void, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let l = lk(owner(this, F_OLELINK));
    if l.moniker.is_null() {
        return MK_E_UNAVAILABLE;
    }
    let bc = bind_context();
    if bc.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = com_call!(
        l.moniker,
        MONIKER_GET_DISPLAY_NAME,
        fn(*mut c_void, *mut c_void, *mut *mut u16) -> HResult,
        bc,
        core::ptr::null_mut(),
        out
    );
    release(bc);
    hr
}

extern "system" fn bind_to_source(this: *mut c_void, _flags: u32, _bc: *mut c_void) -> HResult {
    bind_source(lk(owner(this, F_OLELINK)), b"ole32.dll!IOleLink::BindToSource(the source bound)")
}

/// Asks the source whether it is already running, and binds only then.
/// A source that is not is S_FALSE, which is the whole answer for an
/// unbound link.
extern "system" fn bind_if_running(this: *mut c_void) -> HResult {
    let l = lk(owner(this, F_OLELINK));
    if l.moniker.is_null() {
        return S_FALSE;
    }
    let bc = bind_context();
    if bc.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = com_call!(
        l.moniker,
        MONIKER_IS_RUNNING,
        fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
        bc,
        core::ptr::null_mut(),
        core::ptr::null_mut()
    );
    release(bc);
    if hr != S_OK {
        return S_FALSE;
    }
    bind_source(l, b"ole32.dll!IOleLink::BindIfRunning(the source running)")
}

/// There is never a bound source yet: E_FAIL with the caller's
/// pointer zeroed, as measured.
extern "system" fn get_bound_source(_: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    E_FAIL
}

extern "system" fn unbind_source(_: *mut c_void) -> HResult {
    S_OK
}

extern "system" fn link_update(this: *mut c_void, _bc: *mut c_void) -> HResult {
    bind_source(lk(owner(this, F_OLELINK)), b"ole32.dll!IOleLink::Update(the source bound)")
}

static OLELINK_VTABLE: VTable<14> = VTable([
    link_query as *const (),
    link_add as *const (),
    link_rel as *const (),
    set_update_options as *const (),
    get_update_options as *const (),
    set_source_moniker as *const (),
    get_source_moniker as *const (),
    set_source_display_name as *const (),
    get_source_display_name as *const (),
    bind_to_source as *const (),
    bind_if_running as *const (),
    get_bound_source as *const (),
    unbind_source as *const (),
    link_update as *const (),
]);

// ---- IPersistStorage, and the stream it writes ----

extern "system" fn get_class_id(_: *mut c_void, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, CLSID_STDOLELINK);
    S_OK
}

extern "system" fn is_dirty(this: *mut c_void) -> HResult {
    if lk(owner(this, F_PERSIST)).dirty != 0 { S_OK } else { S_FALSE }
}

/// A link that is already made cannot be made or loaded again.
extern "system" fn init_new(_: *mut c_void, _storage: *mut c_void) -> HResult {
    CO_E_ALREADYINITIALIZED
}

extern "system" fn load(_: *mut c_void, _storage: *mut c_void) -> HResult {
    CO_E_ALREADYINITIALIZED
}

/// The head of the `\1Ole` stream: the OLEStream of MS-OLEDS, as
/// measured field by field. Its last field, the size of the absolute
/// source moniker stream, stays zero here: it counts itself and the
/// moniker's own bytes, so it is not known until the moniker has
/// written them, and `save` goes back for it.
fn write_head(update: u32) -> [u8; 28] {
    let mut head = [0u8; 28];
    // Version 0x02000001.
    head[0..4].copy_from_slice(&0x0200_0001u32.to_le_bytes());
    // Flags: 9 on every link measured.
    head[4..8].copy_from_slice(&9u32.to_le_bytes());
    head[8..12].copy_from_slice(&update.to_le_bytes());
    // Reserved, then the reserved and relative source moniker streams,
    // both empty: a container whose site has no moniker of its own has
    // no relative source to write.
    head
}

/// The tail: a class id indicator with no class, an empty display
/// name, and four more fields a link without a cached presentation
/// leaves empty.
fn write_tail() -> [u8; 52] {
    let mut tail = [0u8; 52];
    tail[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    tail[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    tail
}

fn stream_write(stream: *mut c_void, bytes: &[u8]) -> HResult {
    let mut wrote = 0u32;
    com_call!(
        stream,
        STREAM_WRITE,
        fn(*const u8, u32, *mut u32) -> HResult,
        bytes.as_ptr(),
        bytes.len() as u32,
        &mut wrote
    )
}

fn stream_seek(stream: *mut c_void, to: i64, origin: u32) -> (HResult, u64) {
    let mut at = 0u64;
    let hr = com_call!(stream, STREAM_SEEK, fn(i64, u32, *mut u64) -> HResult, to, origin, &mut at);
    (hr, at)
}

/// The container's moniker, asked for and let go: what a relative
/// source would be worked out from. Windows asks three times on the
/// way through a Save (measured), and a site that has none is no
/// trouble.
fn ask_for_container_moniker(site: *mut c_void) {
    if site.is_null() {
        return;
    }
    let mut moniker: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        site,
        CLIENTSITE_GET_MONIKER,
        fn(u32, u32, *mut *mut c_void) -> HResult,
        OLEGETMONIKER_ONLYIFTHERE,
        OLEWHICHMK_CONTAINER,
        &mut moniker
    );
    if hr >= 0 && !moniker.is_null() {
        release(moniker);
    }
}

/// Save: one stream, `\1Ole`, of 96 + L bytes around the L the source
/// moniker's own Save writes, and `\2OlePres000` destroyed after it.
/// The link stays dirty until SaveCompleted.
extern "system" fn save(this: *mut c_void, storage: *mut c_void, _same: i32) -> HResult {
    if storage.is_null() {
        return E_INVALIDARG;
    }
    let l = lk(owner(this, F_PERSIST));
    if l.moniker.is_null() {
        missing(b"ole32.dll!IPersistStorage::Save(a link with no source)", 0);
        return E_FAIL;
    }
    ask_for_container_moniker(l.site);
    let mut stream: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        storage,
        STORAGE_CREATE_STREAM,
        fn(*const u16, u32, u32, u32, *mut *mut c_void) -> HResult,
        OLE_STREAM.as_ptr(),
        STGM_CREATE | STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
        0,
        0,
        &mut stream
    );
    if hr < 0 {
        return hr;
    }
    if stream.is_null() {
        return E_POINTER;
    }
    // The two moniker streams in the head are worked out here, between
    // the stream and the bytes: the container's moniker once for each.
    ask_for_container_moniker(l.site);
    ask_for_container_moniker(l.site);

    let mut class = GUID_NULL;
    let hr = com_call!(l.moniker, MONIKER_GET_CLASS_ID, fn(*mut Guid) -> HResult, &mut class);
    if hr < 0 {
        release(stream);
        return hr;
    }
    let hr = stream_write(stream, &write_head(l.update));
    if hr < 0 {
        release(stream);
        return hr;
    }
    let hr = stream_write(stream, &class);
    if hr < 0 {
        release(stream);
        return hr;
    }
    // The moniker writes its own bytes where it stands. A moniker that
    // refuses leaves the stream truncated with the size in front of
    // its bytes still zero, and its error is the caller's (measured).
    let hr = com_call!(l.moniker, MONIKER_SAVE, fn(*mut c_void, i32) -> HResult, stream, 1);
    if hr < 0 {
        release(stream);
        return hr;
    }
    let (seek, end) = stream_seek(stream, 0, STREAM_SEEK_CUR);
    if seek < 0 {
        release(stream);
        return seek;
    }
    // AbsoluteSourceMonikerStreamSize counts itself: four, the class
    // id's sixteen, and the moniker's own.
    let moniker_bytes = end.saturating_sub(44) as u32;
    let (seek, _) = stream_seek(stream, 24, STREAM_SEEK_SET);
    if seek < 0 {
        release(stream);
        return seek;
    }
    let hr = stream_write(stream, &(20 + moniker_bytes).to_le_bytes());
    if hr < 0 {
        release(stream);
        return hr;
    }
    let (seek, _) = stream_seek(stream, end as i64, STREAM_SEEK_SET);
    if seek < 0 {
        release(stream);
        return seek;
    }
    let hr = stream_write(stream, &write_tail());
    release(stream);
    if hr < 0 {
        return hr;
    }
    // The presentation the link no longer stands behind. Its failure
    // is nothing: a stream that was never there cannot be destroyed.
    let _ = com_call!(
        storage,
        STORAGE_DESTROY_ELEMENT,
        fn(*const u16) -> HResult,
        OLEPRES_STREAM.as_ptr()
    );
    S_OK
}

/// Saving is not finishing: only this clears the dirty flag. A storage
/// handed in takes the old one's place, held twice as the first was.
extern "system" fn save_completed(this: *mut c_void, storage: *mut c_void) -> HResult {
    let l = lk(owner(this, F_PERSIST));
    if !storage.is_null() {
        if !l.storage.is_null() {
            release(l.storage);
            release(l.storage);
        }
        l.storage = storage;
        add_ref(storage);
        add_ref(storage);
    }
    l.dirty = 0;
    S_OK
}

extern "system" fn hands_off_storage(this: *mut c_void) -> HResult {
    let l = lk(owner(this, F_PERSIST));
    if !l.storage.is_null() {
        release(l.storage);
        release(l.storage);
        l.storage = core::ptr::null_mut();
    }
    S_OK
}

static PERSIST_VTABLE: VTable<10> = VTable([
    persist_query as *const (),
    persist_add as *const (),
    persist_rel as *const (),
    get_class_id as *const (),
    is_dirty as *const (),
    init_new as *const (),
    load as *const (),
    save as *const (),
    save_completed as *const (),
    hands_off_storage as *const (),
]);

// ---- IDataObject: everything needs a running source, except the
//      advise holder, which is the link's own ----

extern "system" fn get_data(
    _: *mut c_void,
    _format: *const c_void,
    _medium: *mut c_void,
) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn get_data_here(
    _: *mut c_void,
    _format: *const c_void,
    _medium: *mut c_void,
) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn query_get_data(_: *mut c_void, _format: *const c_void) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn get_canonical_format_etc(
    _: *mut c_void,
    _format: *const c_void,
    _out: *mut c_void,
) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn data_set_data(
    _: *mut c_void,
    _format: *const c_void,
    _medium: *mut c_void,
    _release: i32,
) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn enum_format_etc(
    _: *mut c_void,
    _direction: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // The formats live in the source's registration.
    put(out, core::ptr::null_mut());
    REGDB_E_CLASSNOTREG
}

extern "system" fn d_advise(
    this: *mut c_void,
    _format: *const c_void,
    _advf: u32,
    sink: *mut c_void,
    out: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let conn = lk(owner(this, F_DATA)).data_advise.add(sink);
    if conn == 0 {
        missing(b"ole32.dll!IDataObject::DAdvise(more sinks than the holder keeps)", SINKS as u64);
        return E_OUTOFMEMORY;
    }
    put(out, conn);
    S_OK
}

extern "system" fn d_unadvise(this: *mut c_void, conn: u32) -> HResult {
    if lk(owner(this, F_DATA)).data_advise.remove(conn) { S_OK } else { OLE_E_NOCONNECTION }
}

extern "system" fn enum_d_advise(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let held = lk(owner(this, F_DATA)).data_advise.held();
    if held != 0 {
        // The enumerator below lists nothing, which is right for an
        // empty holder and a gap for a full one.
        missing(
            b"ole32.dll!IDataObject::EnumDAdvise(sinks the enumerator will not list)",
            held as u64,
        );
    }
    empty_enum(out)
}

static DATA_VTABLE: VTable<12> = VTable([
    data_query as *const (),
    data_add as *const (),
    data_rel as *const (),
    get_data as *const (),
    get_data_here as *const (),
    query_get_data as *const (),
    get_canonical_format_etc as *const (),
    data_set_data as *const (),
    enum_format_etc as *const (),
    d_advise as *const (),
    d_unadvise as *const (),
    enum_d_advise as *const (),
]);

// ---- IViewObject2 ----

/// Drawing is the cached presentation's job, which gala has none of:
/// the row names it, and the answer is the one an unbound link gives.
#[allow(clippy::too_many_arguments)]
extern "system" fn draw(
    _: *mut c_void,
    aspect: u32,
    _index: i32,
    _aspect_info: *mut c_void,
    _target_device: *const c_void,
    _target_dc: usize,
    _dc: usize,
    _bounds: *const c_void,
    _window_bounds: *const c_void,
    _continue_fn: usize,
    _continue_arg: usize,
) -> HResult {
    missing(b"ole32.dll!IViewObject::Draw(no cached presentation)", u64::from(aspect));
    OLE_E_BLANK
}

extern "system" fn get_color_set(
    _: *mut c_void,
    aspect: u32,
    _index: i32,
    _aspect_info: *mut c_void,
    _target_device: *const c_void,
    _target_dc: usize,
    out: *mut *mut c_void,
) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    missing(b"ole32.dll!IViewObject::GetColorSet(no cached presentation)", u64::from(aspect));
    OLE_E_BLANK
}

extern "system" fn freeze(
    _: *mut c_void,
    _aspect: u32,
    _index: i32,
    _aspect_info: *mut c_void,
    _out: *mut u32,
) -> HResult {
    OLE_E_BLANK
}

extern "system" fn unfreeze(_: *mut c_void, _frozen: u32) -> HResult {
    OLE_E_NOCONNECTION
}

/// The view advise is the link's own and is read back whole.
extern "system" fn set_advise(
    this: *mut c_void,
    aspects: u32,
    advf: u32,
    sink: *mut c_void,
) -> HResult {
    let l = lk(owner(this, F_VIEW));
    if !l.view_sink.is_null() {
        release(l.view_sink);
    }
    l.view_sink = sink;
    if !sink.is_null() {
        add_ref(sink);
    }
    l.view_aspects = aspects;
    l.view_advf = advf;
    S_OK
}

extern "system" fn get_advise(
    this: *mut c_void,
    aspects: *mut u32,
    advf: *mut u32,
    sink: *mut *mut c_void,
) -> HResult {
    let l = lk(owner(this, F_VIEW));
    if !aspects.is_null() {
        put(aspects, l.view_aspects);
    }
    if !advf.is_null() {
        put(advf, l.view_advf);
    }
    if !sink.is_null() {
        if !l.view_sink.is_null() {
            add_ref(l.view_sink);
        }
        put(sink, l.view_sink);
    }
    S_OK
}

/// Blank, and the caller's size is left exactly as it was -- where
/// IOleObject::GetExtent zeroes it. Both were measured.
extern "system" fn view_get_extent(
    _: *mut c_void,
    _aspect: u32,
    _index: i32,
    _target_device: *const c_void,
    _size: *mut [i32; 2],
) -> HResult {
    OLE_E_BLANK
}

static VIEW_VTABLE: VTable<10> = VTable([
    view_query as *const (),
    view_add as *const (),
    view_rel as *const (),
    draw as *const (),
    get_color_set as *const (),
    freeze as *const (),
    unfreeze as *const (),
    set_advise as *const (),
    get_advise as *const (),
    view_get_extent as *const (),
]);

// ---- IOleCache2 ----

/// A presentation can be asked for and taken back without a source;
/// what it would hold cannot be fetched, which UpdateCache says.
extern "system" fn cache_add_format(
    this: *mut c_void,
    _format: *const c_void,
    _advf: u32,
    out: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let conn = lk(owner(this, F_CACHE)).cache.add(core::ptr::null_mut());
    if conn == 0 {
        missing(b"ole32.dll!IOleCache::Cache(more formats than the cache keeps)", SINKS as u64);
        return E_OUTOFMEMORY;
    }
    put(out, conn);
    S_OK
}

extern "system" fn uncache(this: *mut c_void, conn: u32) -> HResult {
    if lk(owner(this, F_CACHE)).cache.remove(conn) { S_OK } else { OLE_E_NOCONNECTION }
}

extern "system" fn enum_cache(this: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let held = lk(owner(this, F_CACHE)).cache.held();
    if held != 0 {
        missing(
            b"ole32.dll!IOleCache::EnumCache(formats the enumerator will not list)",
            held as u64,
        );
    }
    empty_enum(out)
}

extern "system" fn init_cache(_: *mut c_void, data: *mut c_void) -> HResult {
    if data.is_null() {
        return E_INVALIDARG;
    }
    missing(b"ole32.dll!IOleCache::InitCache(no cached presentation)", 0);
    E_NOTIMPL
}

extern "system" fn cache_set_data(
    _: *mut c_void,
    _format: *const c_void,
    _medium: *mut c_void,
    _release: i32,
) -> HResult {
    missing(b"ole32.dll!IOleCache::SetData(no cached presentation)", 0);
    E_NOTIMPL
}

extern "system" fn update_cache(
    _: *mut c_void,
    _data: *mut c_void,
    _flags: u32,
    _reserved: *mut c_void,
) -> HResult {
    OLE_E_NOTRUNNING
}

extern "system" fn discard_cache(_: *mut c_void, _options: u32) -> HResult {
    S_OK
}

static CACHE_VTABLE: VTable<10> = VTable([
    cache_query as *const (),
    cache_add as *const (),
    cache_rel as *const (),
    cache_add_format as *const (),
    uncache as *const (),
    enum_cache as *const (),
    init_cache as *const (),
    cache_set_data as *const (),
    update_cache as *const (),
    discard_cache as *const (),
]);

// ---- IRunnableObject ----

extern "system" fn get_running_class(_: *mut c_void, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, CLSID_STDOLELINK);
    S_OK
}

extern "system" fn run(this: *mut c_void, _bc: *mut c_void) -> HResult {
    bind_source(lk(owner(this, F_RUNNABLE)), b"ole32.dll!IRunnableObject::Run(the source bound)")
}

extern "system" fn is_running(_: *mut c_void) -> i32 {
    0
}

extern "system" fn lock_running(_: *mut c_void, _lock: i32, _last_closes: i32) -> HResult {
    S_OK
}

extern "system" fn set_contained_object(_: *mut c_void, _contained: i32) -> HResult {
    S_OK
}

static RUNNABLE_VTABLE: VTable<8> = VTable([
    run_query as *const (),
    run_add as *const (),
    run_rel as *const (),
    get_running_class as *const (),
    run as *const (),
    is_running as *const (),
    lock_running as *const (),
    set_contained_object as *const (),
]);

// ---- An empty IEnumSTATDATA, for the two enumerators that are
//      answered S_OK on a link with nothing cached and nothing
//      advising ----

#[repr(C)]
struct EmptyEnum {
    vtable: *const usize,
    refs: AtomicU32,
}

fn en<'a>(this: *mut EmptyEnum) -> &'a mut EmptyEnum {
    // SAFETY: our object, alive while the caller holds a reference.
    unsafe { &mut *this }
}

extern "system" fn enum_query(
    this: *mut EmptyEnum,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IENUMSTATDATA {
        enum_add(this);
        put(out, this.cast());
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    E_NOINTERFACE
}

extern "system" fn enum_add(this: *mut EmptyEnum) -> u32 {
    en(this).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn enum_rel(this: *mut EmptyEnum) -> u32 {
    let left = en(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(this.cast());
    }
    left
}

extern "system" fn enum_next(
    _: *mut EmptyEnum,
    _want: u32,
    _out: *mut c_void,
    fetched: *mut u32,
) -> HResult {
    if !fetched.is_null() {
        put(fetched, 0);
    }
    S_FALSE
}

extern "system" fn enum_skip(_: *mut EmptyEnum, _count: u32) -> HResult {
    S_FALSE
}

extern "system" fn enum_reset(_: *mut EmptyEnum) -> HResult {
    S_OK
}

extern "system" fn enum_clone(_: *mut EmptyEnum, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    missing(b"ole32.dll!IEnumSTATDATA::Clone(the link's empty enumerator)", 0);
    E_NOTIMPL
}

static ENUM_VTABLE: VTable<7> = VTable([
    enum_query as *const (),
    enum_add as *const (),
    enum_rel as *const (),
    enum_next as *const (),
    enum_skip as *const (),
    enum_reset as *const (),
    enum_clone as *const (),
]);

fn empty_enum(out: *mut *mut c_void) -> HResult {
    put(out, core::ptr::null_mut());
    let p = alloc_zeroed(core::mem::size_of::<EmptyEnum>()).cast::<EmptyEnum>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh zeroed memory of the object's size.
    unsafe {
        p.write(EmptyEnum { vtable: ENUM_VTABLE.0.as_ptr().cast(), refs: AtomicU32::new(1) })
    };
    put(out, p.cast());
    S_OK
}

// ---- The makers ----

/// A link holding `moniker`, `site` and `storage`, with the references
/// Windows holds: one, one, and two.
fn new_link(moniker: *mut c_void, site: *mut c_void, storage: *mut c_void) -> *mut Link {
    let p = alloc_zeroed(core::mem::size_of::<Link>()).cast::<Link>();
    if p.is_null() {
        return p;
    }
    // SAFETY: fresh zeroed memory of the object's size.
    unsafe {
        p.write(Link {
            oleobject_vtable: OLEOBJECT_VTABLE.0.as_ptr().cast(),
            olelink_vtable: OLELINK_VTABLE.0.as_ptr().cast(),
            persist_vtable: PERSIST_VTABLE.0.as_ptr().cast(),
            data_vtable: DATA_VTABLE.0.as_ptr().cast(),
            view_vtable: VIEW_VTABLE.0.as_ptr().cast(),
            cache_vtable: CACHE_VTABLE.0.as_ptr().cast(),
            runnable_vtable: RUNNABLE_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
            moniker,
            site,
            storage,
            // A fresh link updates always, and is dirty until it is
            // saved and told the save went through.
            update: OLEUPDATE_ALWAYS,
            dirty: 1,
            advise: empty_holder(),
            data_advise: empty_holder(),
            cache: empty_holder(),
            view_sink: core::ptr::null_mut(),
            view_aspects: 0,
            view_advf: 0,
        })
    };
    add_ref(moniker);
    if !site.is_null() {
        add_ref(site);
    }
    // Twice: the link's own reference and its cache's.
    add_ref(storage);
    add_ref(storage);
    p
}

/// The arguments, refused in one pass before anything is called --
/// which is what Windows does, measured call by call (the trace is
/// empty for every refusal here).
#[allow(clippy::too_many_arguments)]
fn arguments_refused(
    moniker: *mut c_void,
    render: u32,
    formats: u32,
    advise_flags: *const u32,
    format_etc: *const c_void,
    sink: *mut c_void,
    storage: *mut c_void,
) -> Option<HResult> {
    if moniker.is_null() || storage.is_null() {
        return Some(E_INVALIDARG);
    }
    if render > OLERENDER_ASIS {
        return Some(E_INVALIDARG);
    }
    // OLERENDER_DRAW draws what the source has and takes no format of
    // its own, which it says in its own words rather than
    // E_INVALIDARG.
    if render == OLERENDER_DRAW && formats != 0 {
        return Some(DV_E_CLIPFORMAT);
    }
    if render == OLERENDER_FORMAT && formats == 0 {
        return Some(E_INVALIDARG);
    }
    // The two arrays go with the count, in both directions.
    if (formats != 0) != !format_etc.is_null() || (formats != 0) != !advise_flags.is_null() {
        return Some(E_INVALIDARG);
    }
    // An advise sink is for OLERENDER_FORMAT alone. (rgdwConnection,
    // which is the sink's other half, is never required and never
    // refused.)
    if !sink.is_null() && render != OLERENDER_FORMAT {
        return Some(E_INVALIDARG);
    }
    if formats != 0 && render != OLERENDER_FORMAT {
        // OLERENDER_NONE and OLERENDER_ASIS with formats: not measured
        // on Windows, refused here rather than quietly ignored.
        missing(
            b"ole32.dll!OleCreateLinkEx(formats for a renderopt that takes none)",
            u64::from(render),
        );
        return Some(E_INVALIDARG);
    }
    None
}

/// OleCreateLinkEx(pmkLinkSrc, riid, dwFlags, renderopt, cFormats,
/// rgAdvf, rgFormatEtc, lpAdviseSink, rgdwConnection, pClientSite,
/// pStg, ppvObj): a link to the source `pmkLinkSrc` names, made on
/// `pStg` and handed back as `riid`.
///
/// `dwFlags` (OLECREATE_LEAVERUNNING) is kept by Windows for the bound
/// source, which this module has none of: it records nothing and
/// changes nothing here.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn OleCreateLinkEx(
    moniker: *mut c_void,
    iid: *const Guid,
    _flags: u32,
    render: u32,
    formats: u32,
    advise_flags: *const u32,
    format_etc: *const c_void,
    sink: *mut c_void,
    _connections: *mut u32,
    site: *mut c_void,
    storage: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    // Measured: a NULL out pointer is refused before the caller's
    // pointer is touched or anything is called; every other argument
    // is refused with it zeroed.
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if let Some(hr) =
        arguments_refused(moniker, render, formats, advise_flags, format_etc, sink, storage)
    {
        return hr;
    }

    // The source is bound for IDataObject, which is what a
    // presentation would be rendered from. For OLERENDER_NONE the
    // failure is ignored -- the link is made for a source that is not
    // there -- and for the other three it is fatal in ole32's own
    // words rather than the moniker's.
    let bc = bind_context();
    if bc.is_null() {
        return E_OUTOFMEMORY;
    }
    let mut data: *mut c_void = core::ptr::null_mut();
    let bound = com_call!(
        moniker,
        MONIKER_BIND_TO_OBJECT,
        fn(*mut c_void, *mut c_void, *const Guid, *mut *mut c_void) -> HResult,
        bc,
        core::ptr::null_mut(),
        &IID_IDATAOBJECT,
        &mut data
    );
    release(bc);
    if bound < 0 || data.is_null() {
        if !data.is_null() {
            release(data);
            data = core::ptr::null_mut();
        }
        if render != OLERENDER_NONE {
            return OLE_E_CANT_BINDTOSOURCE;
        }
    }
    if !data.is_null() {
        // A source that does bind is where the cached presentation
        // would come from. gala has none, so the row names the
        // renderopt that went unserved and the link is made blank --
        // which is what its cache methods already say it is.
        if render != OLERENDER_NONE {
            missing(
                b"ole32.dll!OleCreateLinkEx(no presentation cached for a bound source)",
                u64::from(render),
            );
        }
        release(data);
    }
    // Measured: a NULL riid is refused here, after the bind, and not
    // with the other arguments.
    if iid.is_null() {
        return E_INVALIDARG;
    }

    let link = new_link(moniker, site, storage);
    if link.is_null() {
        return E_OUTOFMEMORY;
    }
    // The caller's own interface, asked for before the storage is
    // touched: an interface the link cannot make leaves the storage
    // exactly as it was.
    let mut made: *mut c_void = core::ptr::null_mut();
    let hr = query(link, iid, &mut made);
    if hr < 0 {
        rel(link);
        return hr;
    }
    // Our own reference goes; the caller's is the one query took.
    rel(link);

    // The storage is marked with the link's class. This is the one
    // storage refusal that is fatal, and its error is the caller's,
    // with nothing retained.
    let hr = com_call!(storage, STORAGE_SET_CLASS, fn(*const Guid) -> HResult, &CLSID_STDOLELINK);
    if hr < 0 {
        release(made);
        return hr;
    }
    // What a storage already holding a link would be read from. Both
    // answers are ignored, and the stream is looked for whatever the
    // Stat said.
    let mut stat = [0u8; STATSTG_BYTES];
    let _ = com_call!(
        storage,
        STORAGE_STAT,
        fn(*mut u8, u32) -> HResult,
        stat.as_mut_ptr(),
        STATFLAG_NONAME
    );
    let mut compobj: *mut c_void = core::ptr::null_mut();
    let hr = com_call!(
        storage,
        STORAGE_OPEN_STREAM,
        fn(*const u16, *mut c_void, u32, u32, *mut *mut c_void) -> HResult,
        COMPOBJ_STREAM.as_ptr(),
        core::ptr::null_mut(),
        STGM_SHARE_EXCLUSIVE,
        0,
        &mut compobj
    );
    if hr >= 0 && !compobj.is_null() {
        release(compobj);
    }
    // The container's own moniker, which a relative source would be
    // worked out from, and whether the source is up. Both answers are
    // ignored.
    ask_for_container_moniker(site);
    let bc = bind_context();
    if !bc.is_null() {
        let _ = com_call!(
            moniker,
            MONIKER_IS_RUNNING,
            fn(*mut c_void, *mut c_void, *mut c_void) -> HResult,
            bc,
            core::ptr::null_mut(),
            core::ptr::null_mut()
        );
        release(bc);
    }
    put(out, made);
    S_OK
}

/// OleCreateLink(pmkLinkSrc, riid, renderopt, lpFormatEtc,
/// pClientSite, pStg, ppvObj): OleCreateLinkEx with the three
/// argument slots left out, and measured to be exactly that.
#[unsafe(no_mangle)]
pub extern "system" fn OleCreateLink(
    moniker: *mut c_void,
    iid: *const Guid,
    render: u32,
    format_etc: *const c_void,
    site: *mut c_void,
    storage: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    let formats = u32::from(!format_etc.is_null());
    let advise_flags = if format_etc.is_null() { core::ptr::null() } else { &0u32 };
    OleCreateLinkEx(
        moniker,
        iid,
        0,
        render,
        formats,
        advise_flags,
        format_etc,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        site,
        storage,
        out,
    )
}
