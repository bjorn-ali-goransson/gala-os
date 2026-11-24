/* OleLoad as real Windows behaves, for what Word needs (docs/ROADMAP.md
 * OF): ole32!OleLoad was the last missing export on Word's way up, and
 * the event stream says what a missing one costs. The ledger records
 * `ole32.dll!OleLoad missing export` at 313.121 s of the run; 39 ms
 * later the same thread takes an access violation reading address 0,
 * which Word's handlers catch; it carries on for thirteen seconds and
 * then ends itself with 0x1e198792. A program that delay-loads an export
 * gala does not have is handed a null and calls it.
 *
 * Every expected value below was measured on Windows 11 itself (four
 * probes, in the pull request), and this program is run there as well as
 * under gala. Where the documentation and the machine disagree the
 * machine wins, and four of these are not what the documentation says:
 *
 * - Whether the client site is set before or after the object is loaded
 *   is the object's own choice, not OleLoad's: with a site, OleLoad asks
 *   IOleObject::GetMiscStatus(DVASPECT_CONTENT) and sets the site first
 *   only for OLEMISC_SETCLIENTSITEFIRST. Otherwise the site comes after
 *   IPersistStorage::Load.
 * - GetMiscStatus refusing is not an error: E_NOTIMPL is carried on
 *   from as if no bits were set, and the bits such a call wrote are
 *   not read. SetClientSite refusing, though, is fatal in either
 *   order: that error, the caller's pointer NULL, and an object that
 *   was already loaded let go.
 * - Without a client site none of that happens at all -- no
 *   QueryInterface for IOleObject, no GetMiscStatus, no IOleLink -- even
 *   when the caller asked for IOleObject itself. With one, IOleObject
 *   is required: an object that refuses it is E_NOINTERFACE and is
 *   never loaded, where the same object without a site loads happily.
 * - An object that answers IOleLink is asked, last of all, to
 *   BindIfRunning, and that call's result is ignored.
 *
 * In short:
 *
 * - The order is: the caller's pointer zeroed, IStorage::Stat
 *   (STATFLAG_NONAME) once, the class in the storage activated for the
 *   caller's own interface, QueryInterface for IPersistStorage, Load,
 *   and with a client site the IOleObject calls above around it.
 * - What the caller is left holding: its own interface with one
 *   reference, a storage whose reference count is unchanged (OleLoad
 *   keeps nothing) and a site whose reference count is unchanged (the
 *   object keeps it if it wants). Nothing is cached: loading one storage
 *   twice activates two objects.
 * - The refusals: a NULL storage, a NULL out pointer and a NULL riid are
 *   E_INVALIDARG (the out pointer before the Stat, the riid after it); a
 *   Stat that refuses is its own error with nothing activated;
 *   CLSID_NULL in the storage is REGDB_E_CLASSNOTREG; an object that
 *   cannot make the caller's interface, has no IPersistStorage, or
 *   (with a site) no IOleObject, is E_NOINTERFACE; and a Load or a
 *   SetClientSite that refuses is its own error. Every one of them
 *   leaves the caller's pointer NULL and no object behind.
 *
 * Two things measured on Windows are deliberately not checked here:
 *
 * - A class that is not registered. Windows does not fail: ole32 loads
 *   its own default handler, reading 'Ole', 'CompObj' and 'OlePres000'
 *   out of the storage, and hands back an object that answers
 *   IOleObject, IPersistStorage, IDataObject, IViewObject2, IOleCache2,
 *   IRunnableObject and IMarshal with the storage's class id. gala has
 *   no default handler, so it records the gap and answers
 *   REGDB_E_CLASSNOTREG (ARCHITECTURE "COM: ole32 and oleaut32" names
 *   the limit). A program that needs the handler meets a ledger row
 *   rather than a lie.
 * - Which CLSCTX the activation looks in. It cannot be measured from one
 *   process: a class object registered in the process is found whatever
 *   context it was registered for.
 *
 * Prints "oleload_test: ok" or exits with the number of the failing
 * check. */
#define COBJMACROS
#include <windows.h>
#include <ole2.h>
#include <stdio.h>
#include <string.h>

static int check = 0;
#define CHECK(cond) do { check++; if (!(cond)) { printf("oleload_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess((UINT)check); } } while (0)

#define OLE_IID(d1) {d1, 0x0000, 0x0000, {0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46}}

static const GUID iid_unknown        = OLE_IID(0x00000000);
static const GUID iid_classfactory   = OLE_IID(0x00000001);
static const GUID iid_persiststorage = OLE_IID(0x0000010a);
static const GUID iid_dataobject     = OLE_IID(0x0000010e);
static const GUID iid_oleobject      = OLE_IID(0x00000112);
static const GUID iid_olelink        = OLE_IID(0x0000011d);
static const GUID iid_oleclientsite  = OLE_IID(0x00000118);
static const GUID iid_storage        = OLE_IID(0x0000000b);

/* The class this program registers for itself. */
static const GUID clsid_probe =
    {0x0d0e1a2b, 0x1111, 0x2222, {0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa}};
static const GUID clsid_null = {0, 0, 0, {0, 0, 0, 0, 0, 0, 0, 0}};

#define STG_E_ACCESSDENIED_   ((HRESULT)0x80030005L)
#define REGDB_E_CLASSNOTREG_  ((HRESULT)0x80040154L)
#define OLEMISC_SETCLIENTSITEFIRST_ 0x20000
#define STATFLAG_NONAME_ 1

/* ---- The calls the objects below record, in the order they come ---- */

static char trace[512];
static size_t trace_len;

static void traced(const char *what)
{
    size_t n = strlen(what);
    if (trace_len + n + 2 >= sizeof trace) {
        return;
    }
    if (trace_len) {
        trace[trace_len++] = ' ';
    }
    memcpy(trace + trace_len, what, n);
    trace_len += n;
    trace[trace_len] = 0;
}

static void trace_reset(void)
{
    trace_len = 0;
    trace[0] = 0;
}

/* Prints both sides when they differ: a trace that is nearly right is
   the whole diagnosis, and a bare "check 7 failed" is not. */
static int traced_is(const char *want)
{
    if (strcmp(trace, want) == 0) {
        return 1;
    }
    printf("oleload_test: the calls were not the ones expected\n  wanted: %s\n  got:    %s\n",
           want, trace);
    fflush(stdout);
    return 0;
}

static void traced_qi(const char *prefix, const GUID *iid)
{
    char line[48];
    const char *name = "?";
    if (!iid) {
        name = "null";
    } else if (IsEqualGUID(iid, &iid_unknown)) {
        name = "IUnknown";
    } else if (IsEqualGUID(iid, &iid_classfactory)) {
        name = "IClassFactory";
    } else if (IsEqualGUID(iid, &iid_persiststorage)) {
        name = "IPersistStorage";
    } else if (IsEqualGUID(iid, &iid_dataobject)) {
        name = "IDataObject";
    } else if (IsEqualGUID(iid, &iid_oleobject)) {
        name = "IOleObject";
    } else if (IsEqualGUID(iid, &iid_olelink)) {
        name = "IOleLink";
    } else if (IsEqualGUID(iid, &iid_storage)) {
        name = "IStorage";
    }
    sprintf(line, "%s:%s", prefix, name);
    traced(line);
}

/* ---- The storage the caller supplies: 18 slots, 17 is Stat ---- */

typedef struct {
    void **vtable;
    LONG refs;
    GUID clsid;
    HRESULT stat_hr;
    int stats;
} Storage;

static HRESULT STDMETHODCALLTYPE stg_nope(void *t, void *a, void *b, void *c, void *d)
{
    (void)t; (void)a; (void)b; (void)c; (void)d;
    traced("storage-slot");
    return E_NOTIMPL;
}

static HRESULT STDMETHODCALLTYPE stg_qi(void *this_, const GUID *iid, void **out)
{
    Storage *s = (Storage *)this_;
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &iid_storage))) {
        *out = this_;
        s->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE stg_addref(void *t) { return (ULONG)++((Storage *)t)->refs; }
static ULONG STDMETHODCALLTYPE stg_release(void *t) { return (ULONG)--((Storage *)t)->refs; }

static HRESULT STDMETHODCALLTYPE stg_stat(void *this_, STATSTG *stat, DWORD flag)
{
    Storage *s = (Storage *)this_;
    traced(flag == STATFLAG_NONAME_ ? "stat" : "stat(with the name)");
    s->stats++;
    if (FAILED(s->stat_hr)) {
        return s->stat_hr;
    }
    if (stat) {
        ZeroMemory(stat, sizeof *stat);
        stat->type = STGTY_STORAGE;
        stat->clsid = s->clsid;
    }
    return S_OK;
}

/* The three streams ole32's own default handler reads when the class is
   not registered. gala has no handler, so seeing one of these in a trace
   is news. */
static HRESULT STDMETHODCALLTYPE stg_openstream(void *this_, const wchar_t *name, void *r1,
                                                DWORD mode, DWORD r2, void **out)
{
    char line[64];
    char text[32];
    size_t i = 0;
    (void)this_; (void)r1; (void)mode; (void)r2;
    while (name && name[i] && i < sizeof text - 1) {
        text[i] = (char)name[i];
        i++;
    }
    text[i] = 0;
    sprintf(line, "open:%s", text);
    traced(line);
    if (out) {
        *out = NULL;
    }
    return STG_E_FILENOTFOUND;
}

static void *stg_slots[18];

static void storage_init(Storage *s, const GUID *clsid, HRESULT stat_hr)
{
    int i;
    for (i = 0; i < 18; i++) {
        stg_slots[i] = (void *)stg_nope;
    }
    stg_slots[0] = (void *)stg_qi;
    stg_slots[1] = (void *)stg_addref;
    stg_slots[2] = (void *)stg_release;
    stg_slots[4] = (void *)stg_openstream;
    stg_slots[17] = (void *)stg_stat;
    s->vtable = stg_slots;
    s->refs = 1;
    s->clsid = *clsid;
    s->stat_hr = stat_hr;
    s->stats = 0;
}

/* ---- The object: IPersistStorage (10 slots), IOleObject (24) and
       IOleLink (14), one reference count between them ---- */

typedef struct {
    void **persist;
    void **oleobj;
    void **olelink;
    LONG refs;
    int loads;
    int sites;
    HRESULT load_hr;
    HRESULT site_hr;
    HRESULT misc_hr;
    DWORD misc_status;
    int no_persist;    /* refuse IPersistStorage */
    int no_oleobject;  /* refuse IOleObject (and so the caller's riid) */
    int is_link;       /* answer IOleLink */
} Obj;

static Obj *obj_of_oleobj(void *t) { return (Obj *)((char *)t - offsetof(Obj, oleobj)); }
static Obj *obj_of_olelink(void *t) { return (Obj *)((char *)t - offsetof(Obj, olelink)); }

static HRESULT STDMETHODCALLTYPE obj_qi(void *this_, const GUID *iid, void **out)
{
    Obj *o = (Obj *)this_;
    traced_qi("qi", iid);
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (!iid) {
        return E_POINTER;
    }
    if (IsEqualGUID(iid, &iid_unknown)) {
        *out = o;
        o->refs++;
        return S_OK;
    }
    if (IsEqualGUID(iid, &iid_persiststorage) && !o->no_persist) {
        *out = o;
        o->refs++;
        return S_OK;
    }
    if (IsEqualGUID(iid, &iid_oleobject) && !o->no_oleobject) {
        *out = &o->oleobj;
        o->refs++;
        return S_OK;
    }
    if (IsEqualGUID(iid, &iid_olelink) && o->is_link) {
        *out = &o->olelink;
        o->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static HRESULT STDMETHODCALLTYPE oleobj_qi(void *t, const GUID *i, void **o)
{ return obj_qi(obj_of_oleobj(t), i, o); }
static HRESULT STDMETHODCALLTYPE olelink_qi(void *t, const GUID *i, void **o)
{ return obj_qi(obj_of_olelink(t), i, o); }

static ULONG STDMETHODCALLTYPE obj_addref(void *t) { return (ULONG)++((Obj *)t)->refs; }
static ULONG STDMETHODCALLTYPE obj_release(void *t) { return (ULONG)--((Obj *)t)->refs; }
static ULONG STDMETHODCALLTYPE oleobj_addref(void *t) { return obj_addref(obj_of_oleobj(t)); }
static ULONG STDMETHODCALLTYPE oleobj_release(void *t) { return obj_release(obj_of_oleobj(t)); }
static ULONG STDMETHODCALLTYPE olelink_addref(void *t) { return obj_addref(obj_of_olelink(t)); }
static ULONG STDMETHODCALLTYPE olelink_release(void *t) { return obj_release(obj_of_olelink(t)); }

static HRESULT STDMETHODCALLTYPE obj_nope(void *t, void *a, void *b, void *c, void *d)
{
    (void)t; (void)a; (void)b; (void)c; (void)d;
    traced("object-slot");
    return E_NOTIMPL;
}

static HRESULT STDMETHODCALLTYPE obj_load(void *this_, void *storage)
{
    Obj *o = (Obj *)this_;
    traced(storage ? "load" : "load(no storage)");
    o->loads++;
    return o->load_hr;
}

static HRESULT STDMETHODCALLTYPE oleobj_setclientsite(void *this_, void *site)
{
    Obj *o = obj_of_oleobj(this_);
    traced(site ? "site" : "site(none)");
    o->sites++;
    return o->site_hr;
}

static HRESULT STDMETHODCALLTYPE oleobj_getmiscstatus(void *this_, DWORD aspect, DWORD *status)
{
    Obj *o = obj_of_oleobj(this_);
    traced(aspect == DVASPECT_CONTENT ? "misc" : "misc(another aspect)");
    if (status) {
        *status = o->misc_status;
    }
    return o->misc_hr;
}

static HRESULT STDMETHODCALLTYPE olelink_bindifrunning(void *this_)
{
    (void)this_;
    traced("bind");
    return E_NOTIMPL;
}

static void *persist_slots[10];
static void *oleobj_slots[24];
static void *olelink_slots[14];

static void obj_init(Obj *o)
{
    int i;
    for (i = 0; i < 10; i++) {
        persist_slots[i] = (void *)obj_nope;
    }
    for (i = 0; i < 24; i++) {
        oleobj_slots[i] = (void *)obj_nope;
    }
    for (i = 0; i < 14; i++) {
        olelink_slots[i] = (void *)obj_nope;
    }
    persist_slots[0] = (void *)obj_qi;
    persist_slots[1] = (void *)obj_addref;
    persist_slots[2] = (void *)obj_release;
    persist_slots[6] = (void *)obj_load;          /* IPersistStorage::Load */
    oleobj_slots[0] = (void *)oleobj_qi;
    oleobj_slots[1] = (void *)oleobj_addref;
    oleobj_slots[2] = (void *)oleobj_release;
    oleobj_slots[3] = (void *)oleobj_setclientsite;
    oleobj_slots[22] = (void *)oleobj_getmiscstatus;
    olelink_slots[0] = (void *)olelink_qi;
    olelink_slots[1] = (void *)olelink_addref;
    olelink_slots[2] = (void *)olelink_release;
    olelink_slots[10] = (void *)olelink_bindifrunning;
    o->persist = persist_slots;
    o->oleobj = oleobj_slots;
    o->olelink = olelink_slots;
    o->refs = 0;
    o->loads = 0;
    o->sites = 0;
    o->load_hr = S_OK;
    o->site_hr = S_OK;
    o->misc_hr = S_OK;
    o->misc_status = 0;
    o->no_persist = 0;
    o->no_oleobject = 0;
    o->is_link = 0;
}

/* ---- The class object: five slots ---- */

typedef struct {
    void **vtable;
    LONG refs;
    Obj *object;
} Factory;

static HRESULT STDMETHODCALLTYPE fac_qi(void *this_, const GUID *iid, void **out)
{
    Factory *f = (Factory *)this_;
    traced_qi("fqi", iid);
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &iid_classfactory))) {
        *out = this_;
        f->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE fac_addref(void *t) { return (ULONG)++((Factory *)t)->refs; }
static ULONG STDMETHODCALLTYPE fac_release(void *t) { return (ULONG)--((Factory *)t)->refs; }

static HRESULT STDMETHODCALLTYPE fac_create(void *this_, void *outer, const GUID *iid, void **out)
{
    Factory *f = (Factory *)this_;
    traced_qi("create", iid);
    if (out) {
        *out = NULL;
    }
    if (outer) {
        return CLASS_E_NOAGGREGATION;
    }
    return obj_qi(f->object, iid, out);
}

static HRESULT STDMETHODCALLTYPE fac_lock(void *t, BOOL lock)
{
    (void)t; (void)lock;
    return S_OK;
}

static void *fac_slots[5];

static void factory_init(Factory *f, Obj *object)
{
    fac_slots[0] = (void *)fac_qi;
    fac_slots[1] = (void *)fac_addref;
    fac_slots[2] = (void *)fac_release;
    fac_slots[3] = (void *)fac_create;
    fac_slots[4] = (void *)fac_lock;
    f->vtable = fac_slots;
    f->refs = 1;
    f->object = object;
}

/* ---- The client site: nine slots, none of which should be called ---- */

typedef struct {
    void **vtable;
    LONG refs;
} Site;

static HRESULT STDMETHODCALLTYPE site_nope(void *t, void *a, void *b, void *c, void *d)
{
    (void)t; (void)a; (void)b; (void)c; (void)d;
    traced("site-slot");
    return E_NOTIMPL;
}

static HRESULT STDMETHODCALLTYPE site_qi(void *this_, const GUID *iid, void **out)
{
    Site *s = (Site *)this_;
    traced_qi("sqi", iid);
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &iid_oleclientsite))) {
        *out = this_;
        s->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE site_addref(void *t) { return (ULONG)++((Site *)t)->refs; }
static ULONG STDMETHODCALLTYPE site_release(void *t) { return (ULONG)--((Site *)t)->refs; }

static void *site_slots[9];

static void site_init(Site *s)
{
    int i;
    for (i = 0; i < 9; i++) {
        site_slots[i] = (void *)site_nope;
    }
    site_slots[0] = (void *)site_qi;
    site_slots[1] = (void *)site_addref;
    site_slots[2] = (void *)site_release;
    s->vtable = site_slots;
    s->refs = 1;
}

/* ------------------------------------------------------------------ */

static Storage stg;
static Obj obj;
static Factory fac;
static Site site;

/* One case: a fresh object, a fresh storage, an empty trace. */
static void begin(const GUID *clsid, HRESULT stat_hr)
{
    obj_init(&obj);
    storage_init(&stg, clsid, stat_hr);
    site_init(&site);
    fac.object = &obj;
    trace_reset();
}

int main(void)
{
    HRESULT hr;
    DWORD key = 0;
    void *out = NULL;
    void *second = NULL;

    hr = CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);
    CHECK(hr == S_OK || hr == S_FALSE);
    obj_init(&obj);
    factory_init(&fac, &obj);
    hr = CoRegisterClassObject(&clsid_probe, (IUnknown *)&fac, CLSCTX_INPROC_SERVER,
                               REGCLS_MULTIPLEUSE, &key);
    CHECK(hr == S_OK);

    /* ---- The happy path, no client site, the caller asking for
           IUnknown: Stat, the class activated for IUnknown, and the
           object loaded. ---- */
    begin(&clsid_probe, S_OK);
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_unknown, NULL, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("stat fqi:IClassFactory create:IUnknown qi:IUnknown qi:IPersistStorage load"));
    CHECK(out == (void *)&obj);
    CHECK(obj.refs == 1);          /* one reference, the caller's */
    CHECK(obj.loads == 1);
    CHECK(obj.sites == 0);         /* no site given, none set */
    CHECK(stg.refs == 1);          /* OleLoad keeps nothing of the storage */
    CHECK(stg.stats == 1);         /* exactly one Stat */
    obj_release(&obj);
    CHECK(obj.refs == 0);

    /* ---- The caller asking for IOleObject, still without a site: no
           GetMiscStatus and no IOleLink. The IOleObject side of the
           contract is the site's doing, not the interface's. ---- */
    begin(&clsid_probe, S_OK);
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, NULL, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is(
        "stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IPersistStorage load"));
    CHECK(out == (void *)&obj.oleobj);
    CHECK(obj.refs == 1);
    CHECK(obj.sites == 0);
    oleobj_release(&obj.oleobj);

    /* ---- With a client site, the object asking for nothing special:
           the site is set after the load, and the object is asked
           whether it is a link. ---- */
    begin(&clsid_probe, S_OK);
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc "
                    "qi:IPersistStorage load site qi:IOleLink"));
    CHECK(obj.loads == 1);
    CHECK(obj.sites == 1);
    CHECK(site.refs == 1);         /* OleLoad keeps no reference on the site */
    CHECK(obj.refs == 1);
    oleobj_release(&obj.oleobj);

    /* ---- GetMiscStatus refusing is not an error: the load goes on as
           if no bits were set. ---- */
    begin(&clsid_probe, S_OK);
    obj.misc_hr = E_NOTIMPL;
    obj.misc_status = 0;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc "
                    "qi:IPersistStorage load site qi:IOleLink"));
    CHECK(obj.loads == 1);
    CHECK(obj.sites == 1);
    oleobj_release(&obj.oleobj);

    /* ---- OLEMISC_SETCLIENTSITEFIRST moves the site in front of the
           load: the object's choice, not the caller's. ---- */
    begin(&clsid_probe, S_OK);
    obj.misc_status = OLEMISC_SETCLIENTSITEFIRST_;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc "
                    "site qi:IPersistStorage load qi:IOleLink"));
    CHECK(obj.loads == 1);
    CHECK(obj.sites == 1);
    oleobj_release(&obj.oleobj);

    /* ---- An object that says it is a link is asked to BindIfRunning,
           last of all, and that call's result is ignored. ---- */
    begin(&clsid_probe, S_OK);
    obj.is_link = 1;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == S_OK);             /* BindIfRunning answered E_NOTIMPL */
    CHECK(traced_is("stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc "
                    "qi:IPersistStorage load site qi:IOleLink bind"));
    CHECK(obj.refs == 1);          /* the link reference went back */
    oleobj_release(&obj.oleobj);

    /* ---- GetMiscStatus refusing *and* setting the bit: the bits of a
           call that refused are not read, so the site still comes after
           the load. ---- */
    begin(&clsid_probe, S_OK);
    obj.misc_hr = E_NOTIMPL;
    obj.misc_status = OLEMISC_SETCLIENTSITEFIRST_;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc "
                    "qi:IPersistStorage load site qi:IOleLink"));
    oleobj_release(&obj.oleobj);

    /* ---- SetClientSite refusing after the load ends the call just
           as it does before one: that error, the loaded object let
           go, and the link question never asked. ---- */
    begin(&clsid_probe, S_OK);
    obj.site_hr = E_NOTIMPL;
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == E_NOTIMPL);
    CHECK(traced_is("stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc "
                    "qi:IPersistStorage load site"));
    CHECK(out == NULL);
    CHECK(obj.loads == 1);
    CHECK(obj.refs == 0);

    /* ---- A client site needs an object that can take one: without
           IOleObject the whole load is refused, before it begins.
           The same object without a site loads happily (above). ---- */
    begin(&clsid_probe, S_OK);
    obj.no_oleobject = 1;
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_unknown, (IOleClientSite *)&site, &out);
    CHECK(hr == E_NOINTERFACE);
    CHECK(traced_is("stat fqi:IClassFactory create:IUnknown qi:IUnknown qi:IOleObject"));
    CHECK(out == NULL);
    CHECK(obj.sites == 0);
    CHECK(obj.loads == 0);
    CHECK(obj.refs == 0);

    /* ---- SetClientSite refusing, when the bits asked for it first,
           ends the load: that error, nothing loaded. ---- */
    begin(&clsid_probe, S_OK);
    obj.misc_status = OLEMISC_SETCLIENTSITEFIRST_;
    obj.site_hr = E_NOTIMPL;
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, (IOleClientSite *)&site, &out);
    CHECK(hr == E_NOTIMPL);
    CHECK(traced_is(
        "stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IOleObject misc site"));
    CHECK(out == NULL);
    CHECK(obj.loads == 0);
    CHECK(obj.refs == 0);          /* the object was let go */

    /* ---- An object that cannot make the caller's interface: the
           activation's own error, nothing loaded. ---- */
    begin(&clsid_probe, S_OK);
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_dataobject, NULL, &out);
    CHECK(hr == E_NOINTERFACE);
    CHECK(traced_is("stat fqi:IClassFactory create:IDataObject qi:IDataObject"));
    CHECK(out == NULL);
    CHECK(obj.loads == 0);
    CHECK(obj.refs == 0);

    /* ---- An object with no IPersistStorage: E_NOINTERFACE, and the
           object that was made is let go. ---- */
    begin(&clsid_probe, S_OK);
    obj.no_persist = 1;
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, NULL, &out);
    CHECK(hr == E_NOINTERFACE);
    CHECK(traced_is(
        "stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IPersistStorage"));
    CHECK(out == NULL);
    CHECK(obj.loads == 0);
    CHECK(obj.refs == 0);

    /* ---- Load refusing: its own error, the object let go. ---- */
    begin(&clsid_probe, S_OK);
    obj.load_hr = E_FAIL;
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, NULL, &out);
    CHECK(hr == E_FAIL);
    CHECK(traced_is(
        "stat fqi:IClassFactory create:IOleObject qi:IOleObject qi:IPersistStorage load"));
    CHECK(out == NULL);
    CHECK(obj.loads == 1);
    CHECK(obj.refs == 0);

    /* ---- Stat refusing: its own error, and nothing is activated. ---- */
    begin(&clsid_probe, STG_E_ACCESSDENIED_);
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, NULL, &out);
    CHECK(hr == STG_E_ACCESSDENIED_);
    CHECK(traced_is("stat"));
    CHECK(out == NULL);
    CHECK(obj.refs == 0);

    /* ---- CLSID_NULL in the storage: REGDB_E_CLASSNOTREG, after the
           Stat and before any stream is opened. ---- */
    begin(&clsid_null, S_OK);
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, NULL, &out);
    CHECK(hr == REGDB_E_CLASSNOTREG_);
    CHECK(traced_is("stat"));
    CHECK(out == NULL);

    /* ---- The arguments: a NULL out pointer is refused before the Stat,
           a NULL riid after it, and a NULL storage without one. ---- */
    begin(&clsid_probe, S_OK);
    hr = OleLoad((IStorage *)&stg, &iid_oleobject, NULL, NULL);
    CHECK(hr == E_INVALIDARG);
    CHECK(traced_is(""));
    CHECK(stg.stats == 0);

    begin(&clsid_probe, S_OK);
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad((IStorage *)&stg, NULL, NULL, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(traced_is("stat"));
    CHECK(out == NULL);

    begin(&clsid_probe, S_OK);
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleLoad(NULL, &iid_oleobject, NULL, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(traced_is(""));
    CHECK(out == NULL);

    /* ---- Nothing is cached: one storage loaded twice is two
           activations and two Loads. ---- */
    begin(&clsid_probe, S_OK);
    hr = OleLoad((IStorage *)&stg, &iid_unknown, NULL, &out);
    CHECK(hr == S_OK);
    hr = OleLoad((IStorage *)&stg, &iid_unknown, NULL, &second);
    CHECK(hr == S_OK);
    CHECK(obj.loads == 2);
    CHECK(stg.stats == 2);
    CHECK(obj.refs == 2);
    obj_release(&obj);
    obj_release(&obj);
    CHECK(obj.refs == 0);
    CHECK(stg.refs == 1);

    hr = CoRevokeClassObject(key);
    CHECK(hr == S_OK);
    CoUninitialize();
    printf("oleload_test: ok (%d checks)\n", check);
    fflush(stdout);
    return 0;
}
