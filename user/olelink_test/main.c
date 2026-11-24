/* OleCreateLinkEx and the standard link object, as real Windows behaves,
 * for what Word needs (docs/ROADMAP.md OF). ole32!OleCreateLinkEx is the
 * export Word reaches after OleLoad, and an export gala does not have is
 * a null the program calls: the ledger records the missing row and the
 * thread takes an access violation reading address 0.
 *
 * Every expected value below was measured on Windows 11 itself (seven
 * probes, in the pull request), and this program is run there as well as
 * under gala. The link object is ole32's own -- CLSID_StdOleLink,
 * {00000300-0000-0000-C000-000000000046} -- so its behaviour is the
 * export's behaviour and both are pinned here.
 *
 * The call, in order:
 *
 * - The caller's pointer is zeroed first, before any argument is looked
 *   at.
 * - IMoniker::BindToObject(IDataObject) comes before everything,
 *   including the riid check. With OLERENDER_NONE its failure is
 *   ignored; with DRAW, FORMAT or ASIS it is fatal and the error is
 *   OLE_E_CANT_BINDTOSOURCE, not the moniker's own.
 * - The link object is made, then QueryInterface for the caller's riid.
 *   An interface it cannot make is E_NOINTERFACE with no call on the
 *   storage at all.
 * - IStorage::SetClass(CLSID_StdOleLink). Its failure is fatal: that
 *   error, the caller's pointer NULL, nothing retained.
 * - IStorage::Stat and IStorage::OpenStream("\1CompObj"). Both failures
 *   are ignored, and the second call is made whatever the first
 *   answered.
 * - With a client site, IOleClientSite::GetMoniker; E_NOTIMPL is
 *   harmless.
 * - IMoniker::IsRunning, whose result is ignored. Then S_OK.
 *
 * Nothing is written into the storage at create time and the moniker's
 * Save is never called: the link object holds one reference on the
 * moniker, one on the site and two on the storage -- its own and its
 * cache's -- and lets all of them go with itself. OleCreateLink,
 * without the Ex, does exactly the same.
 *
 * The arguments are refused with E_INVALIDARG: a NULL ppvObj, pmkLinkSrc
 * or pStg; a renderopt outside 0..3; OLERENDER_FORMAT with cFormats 0;
 * rgFormatEtc or rgAdvf NULL when cFormats is non-zero, or non-NULL when
 * it is zero; and a non-NULL lpAdviseSink with anything but
 * OLERENDER_FORMAT. rgdwConnection is never required and never refused.
 * OLERENDER_DRAW with a format is DV_E_CLIPFORMAT. A NULL riid is
 * E_INVALIDARG too, but only after the bind.
 *
 * What the object answers, and what it refuses, is a list rather than a
 * rule: IUnknown, IOleObject, IOleLink, IPersistStorage, IPersist,
 * IDataObject, IViewObject, IViewObject2, IOleCache, IOleCache2 and
 * IRunnableObject yes; IPersistStream, IMarshal, IOleWindow,
 * IExternalConnection, IStdMarshalInfo, IOleCacheControl and
 * IClassFactory no. Its class id is CLSID_StdOleLink, and an unbound
 * link reports itself not running, blank, and not registered -- the
 * three errors it leans on are MK_E_UNAVAILABLE (anything that would
 * have to bind), OLE_E_NOTRUNNING (anything that would have to ask the
 * source for data) and REGDB_E_CLASSNOTREG (anything that would have to
 * read the source's registration).
 *
 * IPersistStorage::Save writes one stream, "\1Ole", of 96 + L bytes
 * where L is what the moniker's own Save writes, destroys
 * "\2OlePres000" after it, and asks the client site for the
 * container's moniker three times on the way -- which is how a relative
 * source would be worked out, and leaves both moniker streams in the
 * head of the stream empty when the site has none. The layout is the
 * OLEStream of MS-OLEDS and every field below was measured by varying
 * one thing at a time; two are worth naming because the obvious reading
 * of the documentation is wrong:
 *
 * - AbsoluteSourceMonikerStreamSize counts itself: it is 20 + L, four
 *   more than the 16-byte class id plus the moniker's bytes.
 * - A moniker whose Save refuses leaves the stream truncated at 44
 *   bytes with that field still zero, and Save returns the moniker's
 *   error.
 *
 * A saved link stays dirty until SaveCompleted, and is clean after it.
 *
 * Two things measured on Windows are deliberately not checked here:
 *
 * - A NULL out pointer on the object's own getters. Windows does not
 *   check them: IOleLink::GetUpdateOptions(NULL) and
 *   GetSourceMoniker(NULL) fault, which cannot be a check in a program
 *   that has to finish. gala refuses them with E_POINTER, as it does
 *   for CreateFileMoniker (ARCHITECTURE "COM: ole32 and oleaut32").
 *   A pointer that is not NULL is pinned below in both directions:
 *   most refusals zero it, and two measured here leave it alone.
 * - What the link does once its source binds. Every call that would
 *   reach a running source is pinned here in its unbound form only; the
 *   bound form needs a server, and Word's first use of the object is
 *   not yet known.
 *
 * Prints "olelink_test: ok" or exits with the number of the failing
 * check. */
#define COBJMACROS
#include <windows.h>
#include <ole2.h>
#include <stdio.h>
#include <string.h>

static int check = 0;
#define CHECK(cond) do { check++; if (!(cond)) { printf("olelink_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess((UINT)check); } } while (0)

#define OLE_IID(d1) {d1, 0x0000, 0x0000, {0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46}}

static const GUID iid_unknown        = OLE_IID(0x00000000);
static const GUID iid_classfactory   = OLE_IID(0x00000001);
static const GUID iid_marshal        = OLE_IID(0x00000003);
static const GUID iid_stdmarshalinfo = OLE_IID(0x00000018);
static const GUID iid_externalconn   = OLE_IID(0x00000019);
static const GUID iid_storage        = OLE_IID(0x0000000b);
static const GUID iid_persist        = OLE_IID(0x0000010c);
static const GUID iid_persiststream  = OLE_IID(0x00000109);
static const GUID iid_persiststorage = OLE_IID(0x0000010a);
static const GUID iid_dataobject     = OLE_IID(0x0000010e);
static const GUID iid_viewobject     = OLE_IID(0x0000010d);
static const GUID iid_viewobject2    = OLE_IID(0x00000127);
static const GUID iid_oleobject      = OLE_IID(0x00000112);
static const GUID iid_olelink        = OLE_IID(0x0000011d);
static const GUID iid_olecache       = OLE_IID(0x0000011e);
static const GUID iid_olecache2      = OLE_IID(0x00000128);
static const GUID iid_olecachecontrol = OLE_IID(0x00000129);
static const GUID iid_runnableobject = OLE_IID(0x00000126);
static const GUID iid_olewindow      = OLE_IID(0x00000114);
static const GUID iid_oleclientsite  = OLE_IID(0x00000118);
static const GUID iid_moniker        = OLE_IID(0x0000000f);

static const GUID clsid_stdolelink   = OLE_IID(0x00000300);
static const GUID clsid_filemoniker  = OLE_IID(0x00000303);
static const GUID clsid_null = {0, 0, 0, {0, 0, 0, 0, 0, 0, 0, 0}};

#define DV_E_CLIPFORMAT_          ((HRESULT)0x8004006AL)
#define OLE_E_CANT_BINDTOSOURCE_  ((HRESULT)0x8004000AL)
#define OLE_E_NOCONNECTION_       ((HRESULT)0x80040004L)
#define OLE_E_NOTRUNNING_         ((HRESULT)0x80040005L)
#define OLE_E_BLANK_              ((HRESULT)0x80040007L)
#define REGDB_E_CLASSNOTREG_      ((HRESULT)0x80040154L)
#define MK_E_UNAVAILABLE_         ((HRESULT)0x800401E3L)
#define MK_E_CANTOPENFILE_        ((HRESULT)0x800401EAL)
#define CO_E_ALREADYINITIALIZED_  ((HRESULT)0x800401F1L)
#define STG_E_ACCESSDENIED_       ((HRESULT)0x80030005L)
#define STG_E_WRITEFAULT_         ((HRESULT)0x8003001DL)
#define STG_E_FILENOTFOUND_       ((HRESULT)0x80030002L)

/* ---- The calls the stubs below record, in the order they come ---- */

static char trace[768];
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
    printf("olelink_test: the calls were not the ones expected\n  wanted: %s\n  got:    %s\n",
           want, trace);
    fflush(stdout);
    return 0;
}

static const char *iid_name(const GUID *iid)
{
    if (!iid) {
        return "null";
    }
    if (IsEqualGUID(iid, &iid_unknown)) return "IUnknown";
    if (IsEqualGUID(iid, &iid_classfactory)) return "IClassFactory";
    if (IsEqualGUID(iid, &iid_dataobject)) return "IDataObject";
    if (IsEqualGUID(iid, &iid_moniker)) return "IMoniker";
    if (IsEqualGUID(iid, &iid_oleobject)) return "IOleObject";
    if (IsEqualGUID(iid, &iid_olelink)) return "IOleLink";
    if (IsEqualGUID(iid, &iid_persiststorage)) return "IPersistStorage";
    if (IsEqualGUID(iid, &iid_storage)) return "IStorage";
    return "?";
}

static void traced_iid(const char *prefix, const GUID *iid)
{
    char line[64];
    sprintf(line, "%s:%s", prefix, iid_name(iid));
    traced(line);
}

/* A stream name with its leading control character written as a digit,
   so a trace is printable: "\1Ole" reads as 1Ole. */
static void traced_name(const char *prefix, const wchar_t *name)
{
    char line[64];
    char text[40];
    size_t i = 0;
    while (name && name[i] && i < sizeof text - 1) {
        wchar_t c = name[i];
        text[i] = (char)(c < 32 ? '0' + (int)c : (char)c);
        i++;
    }
    text[i] = 0;
    sprintf(line, "%s:%s", prefix, text);
    traced(line);
}

/* ---- The stream the storage hands out: a byte buffer with real Seek,
       Write and SetSize, because the link's Save writes the moniker's
       bytes and then goes back to patch the size in front of them ---- */

#define STREAM_MAX 512

typedef struct {
    void **vtable;
    LONG refs;
    unsigned char bytes[STREAM_MAX];
    ULONG len;
    ULONG pos;
} Stream;

static HRESULT STDMETHODCALLTYPE strm_nope(void *t, void *a, void *b, void *c, void *d)
{
    (void)t; (void)a; (void)b; (void)c; (void)d;
    traced("stream-slot");
    return E_NOTIMPL;
}

static HRESULT STDMETHODCALLTYPE strm_qi(void *this_, const GUID *iid, void **out)
{
    Stream *s = (Stream *)this_;
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &IID_IStream)
                || IsEqualGUID(iid, &IID_ISequentialStream))) {
        *out = this_;
        s->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE strm_addref(void *t) { return (ULONG)++((Stream *)t)->refs; }
static ULONG STDMETHODCALLTYPE strm_release(void *t) { return (ULONG)--((Stream *)t)->refs; }

static HRESULT STDMETHODCALLTYPE strm_read(void *this_, void *buf, ULONG want, ULONG *got)
{
    Stream *s = (Stream *)this_;
    ULONG n = s->len > s->pos ? s->len - s->pos : 0;
    if (n > want) {
        n = want;
    }
    if (n) {
        memcpy(buf, s->bytes + s->pos, n);
    }
    s->pos += n;
    if (got) {
        *got = n;
    }
    return n == want ? S_OK : S_FALSE;
}

static HRESULT STDMETHODCALLTYPE strm_write(void *this_, const void *buf, ULONG want, ULONG *got)
{
    Stream *s = (Stream *)this_;
    if (s->pos + want > STREAM_MAX) {
        if (got) {
            *got = 0;
        }
        return STG_E_WRITEFAULT_;
    }
    memcpy(s->bytes + s->pos, buf, want);
    s->pos += want;
    if (s->pos > s->len) {
        s->len = s->pos;
    }
    if (got) {
        *got = want;
    }
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE strm_seek(void *this_, LARGE_INTEGER move, DWORD origin,
                                           ULARGE_INTEGER *out)
{
    Stream *s = (Stream *)this_;
    LONGLONG to = move.QuadPart;
    if (origin == STREAM_SEEK_CUR) {
        to += s->pos;
    } else if (origin == STREAM_SEEK_END) {
        to += s->len;
    }
    if (to < 0 || to > STREAM_MAX) {
        return STG_E_INVALIDFUNCTION;
    }
    s->pos = (ULONG)to;
    if (out) {
        out->QuadPart = (ULONGLONG)to;
    }
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE strm_setsize(void *this_, ULARGE_INTEGER size)
{
    Stream *s = (Stream *)this_;
    if (size.QuadPart > STREAM_MAX) {
        return STG_E_MEDIUMFULL;
    }
    if (size.QuadPart > s->len) {
        memset(s->bytes + s->len, 0, (size_t)size.QuadPart - s->len);
    }
    s->len = (ULONG)size.QuadPart;
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE strm_commit(void *this_, DWORD flags)
{
    (void)this_; (void)flags;
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE strm_stat(void *this_, STATSTG *stat, DWORD flag)
{
    Stream *s = (Stream *)this_;
    (void)flag;
    if (!stat) {
        return E_POINTER;
    }
    ZeroMemory(stat, sizeof *stat);
    stat->type = STGTY_STREAM;
    stat->cbSize.QuadPart = s->len;
    return S_OK;
}

static void *strm_slots[14];

static void stream_init(Stream *s)
{
    int i;
    for (i = 0; i < 14; i++) {
        strm_slots[i] = (void *)strm_nope;
    }
    strm_slots[0] = (void *)strm_qi;
    strm_slots[1] = (void *)strm_addref;
    strm_slots[2] = (void *)strm_release;
    strm_slots[3] = (void *)strm_read;
    strm_slots[4] = (void *)strm_write;
    strm_slots[5] = (void *)strm_seek;
    strm_slots[6] = (void *)strm_setsize;
    strm_slots[8] = (void *)strm_commit;
    strm_slots[12] = (void *)strm_stat;
    s->vtable = strm_slots;
    s->refs = 0;
    s->len = 0;
    s->pos = 0;
    memset(s->bytes, 0, sizeof s->bytes);
}

/* ---- The storage the caller supplies: 18 slots, and one stream of its
       own so the bytes the link writes can be read back ---- */

typedef struct {
    void **vtable;
    LONG refs;
    GUID class_set;
    int classes;
    HRESULT setclass_hr;
    HRESULT stat_hr;
    HRESULT openstream_hr;
    HRESULT createstream_hr;
    Stream stream;          /* whatever CreateStream made, the last one */
    int creates;
    int destroys;
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

static HRESULT STDMETHODCALLTYPE stg_createstream(void *this_, const wchar_t *name, DWORD mode,
                                                  DWORD r1, DWORD r2, void **out)
{
    Storage *s = (Storage *)this_;
    (void)mode; (void)r1; (void)r2;
    traced_name("create", name);
    if (out) {
        *out = NULL;
    }
    if (FAILED(s->createstream_hr)) {
        return s->createstream_hr;
    }
    if (!out) {
        return E_POINTER;
    }
    stream_init(&s->stream);
    s->stream.refs = 1;
    s->creates++;
    *out = &s->stream;
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE stg_openstream(void *this_, const wchar_t *name, void *r1,
                                                DWORD mode, DWORD r2, void **out)
{
    Storage *s = (Storage *)this_;
    (void)r1; (void)mode; (void)r2;
    traced_name("open", name);
    if (out) {
        *out = NULL;
    }
    return s->openstream_hr;
}

static HRESULT STDMETHODCALLTYPE stg_destroyelement(void *this_, const wchar_t *name)
{
    Storage *s = (Storage *)this_;
    traced_name("destroy", name);
    s->destroys++;
    return STG_E_FILENOTFOUND_;
}

static HRESULT STDMETHODCALLTYPE stg_setclass(void *this_, const GUID *clsid)
{
    Storage *s = (Storage *)this_;
    traced("setclass");
    s->classes++;
    if (clsid) {
        s->class_set = *clsid;
    }
    return s->setclass_hr;
}

static HRESULT STDMETHODCALLTYPE stg_stat(void *this_, STATSTG *stat, DWORD flag)
{
    Storage *s = (Storage *)this_;
    (void)flag;
    traced("stat");
    if (FAILED(s->stat_hr)) {
        return s->stat_hr;
    }
    if (stat) {
        ZeroMemory(stat, sizeof *stat);
        stat->type = STGTY_STORAGE;
        stat->clsid = clsid_stdolelink;
    }
    return S_OK;
}

static void *stg_slots[18];

static void storage_init(Storage *s)
{
    int i;
    for (i = 0; i < 18; i++) {
        stg_slots[i] = (void *)stg_nope;
    }
    stg_slots[0] = (void *)stg_qi;
    stg_slots[1] = (void *)stg_addref;
    stg_slots[2] = (void *)stg_release;
    stg_slots[3] = (void *)stg_createstream;
    stg_slots[4] = (void *)stg_openstream;
    stg_slots[12] = (void *)stg_destroyelement;
    stg_slots[15] = (void *)stg_setclass;
    stg_slots[17] = (void *)stg_stat;
    s->vtable = stg_slots;
    s->refs = 1;
    s->class_set = clsid_null;
    s->classes = 0;
    s->setclass_hr = S_OK;
    s->stat_hr = S_OK;
    s->openstream_hr = STG_E_FILENOTFOUND_;
    s->createstream_hr = S_OK;
    s->creates = 0;
    s->destroys = 0;
    stream_init(&s->stream);
}

/* ---- The data object the moniker binds to, when it binds ---- */

typedef struct {
    void **vtable;
    LONG refs;
} Data;

static HRESULT STDMETHODCALLTYPE data_nope(void *t, void *a, void *b, void *c, void *d)
{
    (void)t; (void)a; (void)b; (void)c; (void)d;
    traced("data-slot");
    return E_NOTIMPL;
}

static HRESULT STDMETHODCALLTYPE data_qi(void *this_, const GUID *iid, void **out)
{
    Data *d = (Data *)this_;
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &iid_dataobject))) {
        *out = this_;
        d->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE data_addref(void *t) { return (ULONG)++((Data *)t)->refs; }
static ULONG STDMETHODCALLTYPE data_release(void *t) { return (ULONG)--((Data *)t)->refs; }

static void *data_slots[12];
static Data data_obj;

static void data_init(void)
{
    int i;
    for (i = 0; i < 12; i++) {
        data_slots[i] = (void *)data_nope;
    }
    data_slots[0] = (void *)data_qi;
    data_slots[1] = (void *)data_addref;
    data_slots[2] = (void *)data_release;
    data_obj.vtable = data_slots;
    data_obj.refs = 0;
}

/* ---- The source moniker: 23 slots (IUnknown, IPersist, IPersistStream,
       IMoniker), with knobs for what the link leans on ---- */

typedef struct {
    void **vtable;
    LONG refs;
    HRESULT bind_hr;        /* BindToObject's answer */
    int bind_gives_data;    /* and whether it hands back the data object */
    int binds;
    GUID clsid;
    ULONG save_len;         /* how many bytes Save writes */
    HRESULT save_hr;
    int saves;
    HRESULT isrunning_hr;
    int runnings;
} Mk;

static HRESULT STDMETHODCALLTYPE mk_nope(void *t, void *a, void *b, void *c, void *d)
{
    (void)t; (void)a; (void)b; (void)c; (void)d;
    traced("moniker-slot");
    return E_NOTIMPL;
}

static HRESULT STDMETHODCALLTYPE mk_qi(void *this_, const GUID *iid, void **out)
{
    Mk *m = (Mk *)this_;
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &iid_persist)
                || IsEqualGUID(iid, &iid_persiststream) || IsEqualGUID(iid, &iid_moniker))) {
        *out = this_;
        m->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE mk_addref(void *t) { return (ULONG)++((Mk *)t)->refs; }
static ULONG STDMETHODCALLTYPE mk_release(void *t) { return (ULONG)--((Mk *)t)->refs; }

static HRESULT STDMETHODCALLTYPE mk_getclassid(void *this_, GUID *out)
{
    Mk *m = (Mk *)this_;
    if (!out) {
        return E_POINTER;
    }
    *out = m->clsid;
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE mk_isdirty(void *t) { (void)t; return S_FALSE; }

static HRESULT STDMETHODCALLTYPE mk_save(void *this_, IStream *stream, BOOL clear)
{
    Mk *m = (Mk *)this_;
    unsigned char bytes[64];
    ULONG wrote = 0;
    ULONG i;
    (void)clear;
    traced("moniker-save");
    m->saves++;
    if (FAILED(m->save_hr)) {
        return m->save_hr;
    }
    for (i = 0; i < m->save_len && i < sizeof bytes; i++) {
        bytes[i] = (unsigned char)(0xa0 + i);   /* a ramp, recognisable in a dump */
    }
    if (stream && m->save_len) {
        IStream_Write(stream, bytes, m->save_len, &wrote);
    }
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE mk_getsizemax(void *this_, ULARGE_INTEGER *out)
{
    Mk *m = (Mk *)this_;
    if (out) {
        out->QuadPart = m->save_len;
    }
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE mk_bindtoobject(void *this_, IBindCtx *bc, IMoniker *left,
                                                 const GUID *iid, void **out)
{
    Mk *m = (Mk *)this_;
    (void)bc; (void)left;
    traced_iid("bind", iid);
    m->binds++;
    if (out) {
        *out = NULL;
    }
    if (FAILED(m->bind_hr)) {
        return m->bind_hr;
    }
    if (!m->bind_gives_data || !out || !iid || !IsEqualGUID(iid, &iid_dataobject)) {
        return E_NOINTERFACE;
    }
    *out = &data_obj;
    data_obj.refs++;
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE mk_isrunning(void *this_, IBindCtx *bc, IMoniker *left,
                                              IMoniker *newly)
{
    Mk *m = (Mk *)this_;
    (void)bc; (void)left; (void)newly;
    traced("isrunning");
    m->runnings++;
    return m->isrunning_hr;
}

static HRESULT STDMETHODCALLTYPE mk_getdisplayname(void *this_, IBindCtx *bc, IMoniker *left,
                                                   wchar_t **out)
{
    static const wchar_t name[] = L"the source";
    (void)this_; (void)bc; (void)left;
    traced("displayname");
    if (!out) {
        return E_POINTER;
    }
    *out = CoTaskMemAlloc(sizeof name);
    if (!*out) {
        return E_OUTOFMEMORY;
    }
    memcpy(*out, name, sizeof name);
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE mk_hash(void *this_, DWORD *out)
{
    (void)this_;
    if (out) {
        *out = 0x1234;
    }
    return S_OK;
}

static HRESULT STDMETHODCALLTYPE mk_isequal(void *this_, IMoniker *other)
{
    (void)this_;
    (void)other;
    return S_FALSE;
}

static HRESULT STDMETHODCALLTYPE mk_issystemmoniker(void *this_, DWORD *out)
{
    (void)this_;
    if (out) {
        *out = MKSYS_FILEMONIKER;
    }
    return S_OK;
}

static void *mk_slots[23];

static void moniker_init(Mk *m)
{
    int i;
    for (i = 0; i < 23; i++) {
        mk_slots[i] = (void *)mk_nope;
    }
    mk_slots[0] = (void *)mk_qi;
    mk_slots[1] = (void *)mk_addref;
    mk_slots[2] = (void *)mk_release;
    mk_slots[3] = (void *)mk_getclassid;
    mk_slots[4] = (void *)mk_isdirty;
    mk_slots[6] = (void *)mk_save;
    mk_slots[7] = (void *)mk_getsizemax;
    mk_slots[8] = (void *)mk_bindtoobject;
    mk_slots[13] = (void *)mk_isequal;
    mk_slots[14] = (void *)mk_hash;
    mk_slots[15] = (void *)mk_isrunning;
    mk_slots[20] = (void *)mk_getdisplayname;
    mk_slots[22] = (void *)mk_issystemmoniker;
    m->vtable = mk_slots;
    m->refs = 1;
    m->bind_hr = MK_E_UNAVAILABLE_;
    m->bind_gives_data = 0;
    m->binds = 0;
    m->clsid = clsid_filemoniker;
    m->save_len = 4;
    m->save_hr = S_OK;
    m->saves = 0;
    m->isrunning_hr = S_FALSE;
    m->runnings = 0;
}

/* ---- The client site: 9 slots ---- */

typedef struct {
    void **vtable;
    LONG refs;
    HRESULT getmoniker_hr;
    int monikers;
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

static HRESULT STDMETHODCALLTYPE site_getmoniker(void *this_, DWORD assign, DWORD which,
                                                 IMoniker **out)
{
    Site *s = (Site *)this_;
    (void)assign; (void)which;
    traced("getmoniker");
    s->monikers++;
    if (out) {
        *out = NULL;
    }
    return s->getmoniker_hr;
}

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
    site_slots[4] = (void *)site_getmoniker;
    s->vtable = site_slots;
    s->refs = 1;
    s->getmoniker_hr = E_NOTIMPL;
    s->monikers = 0;
}

/* ---- The advise sink: 8 slots, nothing but a reference count ---- */

typedef struct {
    void **vtable;
    LONG refs;
} Sink;

static HRESULT STDMETHODCALLTYPE sink_qi(void *this_, const GUID *iid, void **out)
{
    Sink *s = (Sink *)this_;
    if (!out) {
        return E_POINTER;
    }
    *out = NULL;
    if (iid && (IsEqualGUID(iid, &iid_unknown) || IsEqualGUID(iid, &IID_IAdviseSink))) {
        *out = this_;
        s->refs++;
        return S_OK;
    }
    return E_NOINTERFACE;
}

static ULONG STDMETHODCALLTYPE sink_addref(void *t) { return (ULONG)++((Sink *)t)->refs; }
static ULONG STDMETHODCALLTYPE sink_release(void *t) { return (ULONG)--((Sink *)t)->refs; }
static void STDMETHODCALLTYPE sink_void(void *t, void *a, void *b)
{ (void)t; (void)a; (void)b; traced("sink-slot"); }

static void *sink_slots[8];
static Sink sink;

static void sink_init(void)
{
    int i;
    for (i = 0; i < 8; i++) {
        sink_slots[i] = (void *)sink_void;
    }
    sink_slots[0] = (void *)sink_qi;
    sink_slots[1] = (void *)sink_addref;
    sink_slots[2] = (void *)sink_release;
    sink.vtable = sink_slots;
    sink.refs = 1;
}

/* ---- The pieces every case starts from ---- */

static Mk mk;
static Storage stg;
static Site site;

static void begin(void)
{
    moniker_init(&mk);
    storage_init(&stg);
    site_init(&site);
    data_init();
    trace_reset();
}

static void a_format(FORMATETC *fe, CLIPFORMAT cf)
{
    ZeroMemory(fe, sizeof *fe);
    fe->cfFormat = cf;
    fe->dwAspect = DVASPECT_CONTENT;
    fe->lindex = -1;
    fe->tymed = TYMED_MFPICT;
}

/* A plain link, the way every method case wants one. */
static IUnknown *a_link(void)
{
    void *out = NULL;
    HRESULT hr;
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, (IOleClientSite *)&site, (IStorage *)&stg, &out);
    if (hr != S_OK || !out) {
        printf("olelink_test: the link would not even be made (0x%08lx)\n", (unsigned long)hr);
        fflush(stdout);
        ExitProcess(250);
    }
    trace_reset();
    return (IUnknown *)out;
}

static void *link_as(IUnknown *link, const GUID *iid)
{
    void *p = NULL;
    HRESULT hr = IUnknown_QueryInterface(link, iid, &p);
    if (hr != S_OK || !p) {
        printf("olelink_test: the link refuses %s (0x%08lx)\n", iid_name(iid), (unsigned long)hr);
        fflush(stdout);
        ExitProcess(251);
    }
    return p;
}

/* Does the object answer this interface? */
static int answers(IUnknown *link, const GUID *iid)
{
    void *p = NULL;
    HRESULT hr = IUnknown_QueryInterface(link, iid, &p);
    if (hr != S_OK || !p) {
        printf("olelink_test: %s was refused (0x%08lx)\n", iid_name(iid),
               (unsigned long)hr);
        fflush(stdout);
        return 0;
    }
    IUnknown_Release((IUnknown *)p);
    return 1;
}

/* And does it refuse this one, cleanly -- E_NOINTERFACE with the
   caller's pointer zeroed, not a vtable of the wrong shape? */
static int refuses(IUnknown *link, const GUID *iid)
{
    void *p = (void *)(ULONG_PTR)0xdeadbeef;
    HRESULT hr = IUnknown_QueryInterface(link, iid, &p);
    if (hr == E_NOINTERFACE && p == NULL) {
        return 1;
    }
    printf("olelink_test: %s was not refused cleanly (0x%08lx, %p)\n", iid_name(iid),
           (unsigned long)hr, p);
    fflush(stdout);
    return 0;
}

int main(void)
{
    HRESULT hr;
    void *out = NULL;
    IUnknown *link;
    FORMATETC formats[2];
    DWORD advf[2] = {0, 0};
    DWORD conns[2] = {0xcccccccc, 0xcccccccc};

    hr = OleInitialize(NULL);
    CHECK(hr == S_OK || hr == S_FALSE);
    sink_init();

    a_format(&formats[0], CF_METAFILEPICT);
    a_format(&formats[1], CF_TEXT);

    /* ================ the arguments ================ */

    /* A NULL out pointer: refused before anything is touched. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, NULL);
    CHECK(hr == E_INVALIDARG);
    CHECK(traced_is(""));
    CHECK(mk.binds == 0);

    /* A NULL moniker, and a NULL storage: the caller's pointer is
       zeroed first either way. */
    begin();
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx(NULL, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL, NULL, NULL,
                         (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(out == NULL);
    CHECK(traced_is(""));

    begin();
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, NULL, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(out == NULL);
    CHECK(traced_is(""));
    CHECK(mk.binds == 0);

    /* renderopt is 0..3 and nothing else. */
    begin();
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, 4, 0, NULL, NULL, NULL, NULL, NULL,
                         (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(out == NULL);
    CHECK(traced_is(""));

    /* OLERENDER_FORMAT needs a format. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_FORMAT, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(traced_is(""));

    /* The three arrays go together: each of rgFormatEtc and rgAdvf is
       required exactly when cFormats is non-zero, and refused
       otherwise. rgdwConnection is neither. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_FORMAT, 1, advf, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);

    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_FORMAT, 1, NULL, formats,
                         NULL, NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);

    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, formats, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);

    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, advf, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);

    /* An advise sink is for OLERENDER_FORMAT alone. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL,
                         (IAdviseSink *)&sink, NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(traced_is(""));

    /* OLERENDER_DRAW takes no format, and says so in its own words. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_DRAW, 1, advf, formats, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == DV_E_CLIPFORMAT_);
    CHECK(traced_is(""));

    /* A NULL riid is refused too -- but only after the bind, which is
       the one argument check that costs the caller a call. */
    begin();
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx((IMoniker *)&mk, NULL, 0, OLERENDER_NONE, 0, NULL, NULL, NULL, NULL,
                         NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_INVALIDARG);
    CHECK(out == NULL);
    CHECK(traced_is("bind:IDataObject"));

    /* ================ the call ================ */

    /* OLERENDER_NONE, no site: the bind is tried and its failure
       ignored, the object is made, the storage is marked, and the
       moniker is asked whether it is running. Nothing is written. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == S_OK);
    CHECK(out != NULL);
    CHECK(traced_is("bind:IDataObject setclass stat open:1CompObj isrunning"));
    CHECK(IsEqualGUID(&stg.class_set, &clsid_stdolelink));
    CHECK(stg.creates == 0);       /* nothing written at create time */
    CHECK(mk.saves == 0);          /* and the moniker is not saved */
    CHECK(mk.refs == 2);           /* the link holds the moniker */
    CHECK(stg.refs == 3);          /* and the storage twice: the link's own
                                      reference and its cache's */
    IUnknown_Release((IUnknown *)out);
    CHECK(mk.refs == 1);           /* both let go with the object */
    CHECK(stg.refs == 1);

    /* With a client site: the site is asked for a moniker, and its
       E_NOTIMPL is no trouble. The site is held too. */
    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, (IOleClientSite *)&site, (IStorage *)&stg, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("bind:IDataObject setclass stat open:1CompObj getmoniker isrunning"));
    CHECK(site.refs == 2);
    IUnknown_Release((IUnknown *)out);
    CHECK(site.refs == 1);

    /* OleCreateLink is the same call with the three argument slots left
       out. */
    begin();
    hr = OleCreateLink((IMoniker *)&mk, &iid_unknown, OLERENDER_NONE, NULL, NULL,
                       (IStorage *)&stg, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("bind:IDataObject setclass stat open:1CompObj isrunning"));
    IUnknown_Release((IUnknown *)out);

    /* An interface the link cannot make: E_NOINTERFACE, and the
       storage is never touched. */
    begin();
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_classfactory, 0, OLERENDER_NONE, 0, NULL, NULL,
                         NULL, NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == E_NOINTERFACE);
    CHECK(out == NULL);
    CHECK(traced_is("bind:IDataObject"));
    CHECK(stg.classes == 0);
    CHECK(mk.refs == 1);

    /* SetClass refusing is fatal, and its error is the caller's. */
    begin();
    stg.setclass_hr = STG_E_ACCESSDENIED_;
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == STG_E_ACCESSDENIED_);
    CHECK(out == NULL);
    CHECK(traced_is("bind:IDataObject setclass"));
    CHECK(mk.refs == 1);           /* nothing retained */
    CHECK(stg.refs == 1);

    /* Stat refusing is not: the link is made all the same, and the
       stream behind the Stat is still looked for. */
    begin();
    stg.stat_hr = STG_E_ACCESSDENIED_;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("bind:IDataObject setclass stat open:1CompObj isrunning"));
    IUnknown_Release((IUnknown *)out);

    /* Nor is the stream being there rather than missing: it is opened
       and let go, and nothing is read out of it that matters here. */
    begin();
    stg.openstream_hr = STG_E_ACCESSDENIED_;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == S_OK);
    CHECK(traced_is("bind:IDataObject setclass stat open:1CompObj isrunning"));
    IUnknown_Release((IUnknown *)out);

    /* Nor is IsRunning saying the source is up: the result is ignored. */
    begin();
    mk.isrunning_hr = S_OK;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == S_OK);
    CHECK(mk.runnings == 1);
    IUnknown_Release((IUnknown *)out);

    /* A bind that fails is only ignored for OLERENDER_NONE. The other
       three need the source, and say OLE_E_CANT_BINDTOSOURCE rather
       than passing the moniker's error on. */
    begin();
    out = (void *)(ULONG_PTR)0xdeadbeef;
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_DRAW, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == OLE_E_CANT_BINDTOSOURCE_);
    CHECK(out == NULL);
    CHECK(traced_is("bind:IDataObject"));

    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_ASIS, 0, NULL, NULL, NULL,
                         NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == OLE_E_CANT_BINDTOSOURCE_);

    begin();
    hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_FORMAT, 1, advf, formats,
                         NULL, NULL, NULL, (IStorage *)&stg, &out);
    CHECK(hr == OLE_E_CANT_BINDTOSOURCE_);

    /* ================ the object ================ */

    link = a_link();

    CHECK(answers(link, &iid_unknown));
    CHECK(answers(link, &iid_oleobject));
    CHECK(answers(link, &iid_olelink));
    CHECK(answers(link, &iid_persiststorage));
    CHECK(answers(link, &iid_persist));
    CHECK(answers(link, &iid_dataobject));
    CHECK(answers(link, &iid_viewobject));
    CHECK(answers(link, &iid_viewobject2));
    CHECK(answers(link, &iid_olecache));
    CHECK(answers(link, &iid_olecache2));
    CHECK(answers(link, &iid_runnableobject));

    /* And what it refuses, which is as much of the contract: a program
       that asks for IPersistStream must get E_NOINTERFACE, not a
       vtable with the wrong shape. */
    CHECK(refuses(link, &iid_persiststream));
    CHECK(refuses(link, &iid_marshal));
    CHECK(refuses(link, &iid_olewindow));
    CHECK(refuses(link, &iid_externalconn));
    CHECK(refuses(link, &iid_stdmarshalinfo));
    CHECK(refuses(link, &iid_olecachecontrol));
    CHECK(refuses(link, &iid_classfactory));
    IUnknown_Release(link);

    /* ---- IOleLink ---- */
    {
        IOleLink *l;
        DWORD option = 0;
        IMoniker *got = NULL;
        wchar_t *name = NULL;

        link = a_link();
        l = link_as(link, &iid_olelink);

        /* A fresh link updates always, and takes the three options the
           header names and nothing else. */
        CHECK(IOleLink_GetUpdateOptions(l, &option) == S_OK);
        CHECK(option == OLEUPDATE_ALWAYS);
        CHECK(IOleLink_SetUpdateOptions(l, OLEUPDATE_ONCALL) == S_OK);
        CHECK(IOleLink_GetUpdateOptions(l, &option) == S_OK);
        CHECK(option == OLEUPDATE_ONCALL);
        CHECK(IOleLink_SetUpdateOptions(l, 99) == E_INVALIDARG);
        CHECK(IOleLink_GetUpdateOptions(l, &option) == S_OK);
        CHECK(option == OLEUPDATE_ONCALL);    /* the refusal changed nothing */
        CHECK(IOleLink_SetUpdateOptions(l, OLEUPDATE_ALWAYS) == S_OK);

        /* The moniker handed back is the very pointer given, with a
           reference of its own. */
        CHECK(IOleLink_GetSourceMoniker(l, &got) == S_OK);
        CHECK(got == (IMoniker *)&mk);
        CHECK(mk.refs == 3);                  /* the link's, and this one */
        IMoniker_Release(got);

        /* The display name is the moniker's, fetched when asked. */
        trace_reset();
        CHECK(IOleLink_GetSourceDisplayName(l, &name) == S_OK);
        CHECK(name != NULL && wcscmp(name, L"the source") == 0);
        CHECK(traced_is("displayname"));
        CoTaskMemFree(name);

        /* Setting a name parses it: a file that is not there is the
           moniker layer's own refusal. */
        CHECK(IOleLink_SetSourceDisplayName(l, L"C:\\no\\such\\source.docx")
              == MK_E_CANTOPENFILE_);
        CHECK(IOleLink_SetSourceDisplayName(l, NULL) == E_INVALIDARG);

        /* Setting the moniker back is allowed, and so, oddly, is
           setting none: the link takes a NULL source without
           complaint. */
        CHECK(IOleLink_SetSourceMoniker(l, (IMoniker *)&mk, &clsid_null) == S_OK);
        CHECK(IOleLink_SetSourceMoniker(l, NULL, &clsid_null) == S_OK);

        IOleLink_Release(l);
        IUnknown_Release(link);
    }

    /* Everything that would have to reach the source, on a link whose
       source will not bind. */
    {
        IOleLink *l;
        IUnknown *bound = (IUnknown *)(ULONG_PTR)0xdeadbeef;

        link = a_link();
        l = link_as(link, &iid_olelink);

        CHECK(IOleLink_BindIfRunning(l) == S_FALSE);
        CHECK(IOleLink_UnbindSource(l) == S_OK);
        CHECK(IOleLink_GetBoundSource(l, &bound) == E_FAIL);
        CHECK(bound == NULL);
        CHECK(IOleLink_BindToSource(l, 0, NULL) == MK_E_UNAVAILABLE_);
        CHECK(IOleLink_Update(l, NULL) == MK_E_UNAVAILABLE_);

        IOleLink_Release(l);
        IUnknown_Release(link);
    }

    /* ---- IOleObject ---- */
    {
        IOleObject *o;
        GUID id;
        DWORD status = 0xcccccccc;
        IOleClientSite *got_site = NULL;
        wchar_t *type = (wchar_t *)(ULONG_PTR)0xdeadbeef;
        IMoniker *got_mk = NULL;
        SIZEL size;
        IEnumOLEVERB *verbs = (IEnumOLEVERB *)(ULONG_PTR)0xdeadbeef;
        IEnumSTATDATA *advises = (IEnumSTATDATA *)(ULONG_PTR)0xdeadbeef;
        IDataObject *clip = NULL;
        DWORD conn = 0xcccccccc;
        LOGPALETTE palette;

        link = a_link();
        o = link_as(link, &iid_oleobject);

        /* An unbound link has no class of its own to report. */
        CHECK(IOleObject_GetUserClassID(o, &id) == S_OK);
        CHECK(IsEqualGUID(&id, &clsid_null));

        /* The status bits and the user type both live in the source's
           registration, which is not there: the error comes back and
           the bits are written anyway. */
        CHECK(IOleObject_GetMiscStatus(o, DVASPECT_CONTENT, &status) == REGDB_E_CLASSNOTREG_);
        CHECK(status == 0x50);
        status = 0xcccccccc;
        CHECK(IOleObject_GetMiscStatus(o, DVASPECT_ICON, &status) == REGDB_E_CLASSNOTREG_);
        CHECK(status == 0x50);
        CHECK(IOleObject_GetUserType(o, USERCLASSTYPE_FULL, &type) == REGDB_E_CLASSNOTREG_);
        CHECK(type == NULL);
        CHECK(IOleObject_EnumVerbs(o, &verbs) == REGDB_E_CLASSNOTREG_);
        CHECK(verbs == NULL);

        /* The site is the one given, with a reference of its own, and
           can be taken away. */
        CHECK(IOleObject_GetClientSite(o, &got_site) == S_OK);
        CHECK(got_site == (IOleClientSite *)&site);
        IOleClientSite_Release(got_site);
        CHECK(IOleObject_SetClientSite(o, NULL) == S_OK);
        CHECK(site.refs == 1);
        CHECK(IOleObject_SetClientSite(o, (IOleClientSite *)&site) == S_OK);
        CHECK(site.refs == 2);

        /* The container's moniker is the site's business, and the
           site's answer is passed on as it stands. */
        trace_reset();
        CHECK(IOleObject_GetMoniker(o, OLEGETMONIKER_ONLYIFTHERE, OLEWHICHMK_CONTAINER, &got_mk)
              == E_NOTIMPL);
        CHECK(traced_is("getmoniker"));

        /* Names and a palette are taken and forgotten; a moniker of
           the object's own is taken too. */
        CHECK(IOleObject_SetHostNames(o, L"a container", L"a document") == S_OK);
        ZeroMemory(&palette, sizeof palette);
        palette.palVersion = 0x300;
        palette.palNumEntries = 1;
        CHECK(IOleObject_SetColorScheme(o, &palette) == S_OK);
        CHECK(IOleObject_SetMoniker(o, OLEWHICHMK_OBJREL, NULL) == S_OK);

        /* An unbound link is blank, and cannot be asked to draw, show
           or refresh itself. */
        size.cx = -1;
        size.cy = -1;
        CHECK(IOleObject_GetExtent(o, DVASPECT_CONTENT, &size) == OLE_E_BLANK_);
        CHECK(size.cx == 0 && size.cy == 0);
        size.cx = 2540;
        size.cy = 2540;
        CHECK(IOleObject_SetExtent(o, DVASPECT_CONTENT, &size) == E_FAIL);
        CHECK(IOleObject_IsUpToDate(o) == MK_E_UNAVAILABLE_);
        CHECK(IOleObject_Update(o) == MK_E_UNAVAILABLE_);
        CHECK(IOleObject_DoVerb(o, OLEIVERB_SHOW, NULL, (IOleClientSite *)&site, 0, NULL, NULL)
              == MK_E_UNAVAILABLE_);
        CHECK(IOleObject_InitFromData(o, NULL, TRUE, 0) == OLE_E_NOTRUNNING_);
        CHECK(IOleObject_GetClipboardData(o, 0, &clip) == OLE_E_NOTRUNNING_);
        CHECK(clip == NULL);

        /* Advising works without a source: the holder is the link's own,
           and the first Advise makes it. Before there is one,
           EnumAdvise is E_FAIL; once there is, it is E_NOTIMPL -- the
           holder keeps the sinks but will not list them. The two
           refusals differ in the caller's pointer as well: the link
           leaves it alone, the holder zeroes it. */
        CHECK(IOleObject_EnumAdvise(o, &advises) == E_FAIL);
        CHECK(advises == (IEnumSTATDATA *)(ULONG_PTR)0xdeadbeef);   /* left as it was */
        CHECK(IOleObject_Advise(o, (IAdviseSink *)&sink, &conn) == S_OK);
        CHECK(conn == 1);
        CHECK(IOleObject_EnumAdvise(o, &advises) == E_NOTIMPL);
        CHECK(advises == NULL);
        CHECK(IOleObject_Unadvise(o, conn) == S_OK);
        CHECK(IOleObject_EnumAdvise(o, &advises) == E_NOTIMPL);  /* the holder stays */
        CHECK(IOleObject_Unadvise(o, 77) == OLE_E_NOCONNECTION_);

        /* Closing an unbound link is nothing to do, and done twice. */
        CHECK(IOleObject_Close(o, OLECLOSE_NOSAVE) == S_OK);
        CHECK(IOleObject_Close(o, OLECLOSE_NOSAVE) == S_OK);

        IOleObject_Release(o);
        IUnknown_Release(link);
    }

    /* ---- IRunnableObject ---- */
    {
        IRunnableObject *r;
        GUID id;

        link = a_link();
        r = link_as(link, &iid_runnableobject);

        CHECK(IRunnableObject_IsRunning(r) == FALSE);
        CHECK(IRunnableObject_GetRunningClass(r, &id) == S_OK);
        CHECK(IsEqualGUID(&id, &clsid_stdolelink));
        CHECK(IRunnableObject_LockRunning(r, TRUE, FALSE) == S_OK);
        CHECK(IRunnableObject_LockRunning(r, FALSE, FALSE) == S_OK);
        CHECK(IRunnableObject_SetContainedObject(r, TRUE) == S_OK);
        CHECK(IRunnableObject_Run(r, NULL) == MK_E_UNAVAILABLE_);

        IRunnableObject_Release(r);
        IUnknown_Release(link);
    }

    /* ---- IPersistStorage, and the stream it writes ---- */
    {
        IPersistStorage *p;
        GUID id;
        const unsigned char *b;
        ULONG i;

        link = a_link();
        p = link_as(link, &iid_persiststorage);

        CHECK(IPersistStorage_GetClassID(p, &id) == S_OK);
        CHECK(IsEqualGUID(&id, &clsid_stdolelink));
        CHECK(IPersistStorage_IsDirty(p) == S_OK);

        /* A link that is already made cannot be made again. */
        CHECK(IPersistStorage_InitNew(p, (IStorage *)&stg) == CO_E_ALREADYINITIALIZED_);
        CHECK(IPersistStorage_Load(p, (IStorage *)&stg) == CO_E_ALREADYINITIALIZED_);
        CHECK(IPersistStorage_Save(p, NULL, TRUE) == E_INVALIDARG);

        /* Save writes one stream and clears one presentation -- and
           asks the client site for the container's moniker three
           times on the way, which is how the relative source would be
           worked out. A site that has none (E_NOTIMPL) leaves the two
           moniker streams in the head of the stream empty, below. */
        trace_reset();
        CHECK(IPersistStorage_Save(p, (IStorage *)&stg, TRUE) == S_OK);
        CHECK(traced_is("getmoniker create:1Ole getmoniker getmoniker moniker-save "
                        "destroy:2OlePres000"));
        CHECK(site.monikers == 4);     /* three here, one when it was made */
        CHECK(stg.creates == 1);
        CHECK(mk.saves == 1);

        /* 96 bytes of frame around the moniker's own four. */
        CHECK(stg.stream.len == 96 + 4);
        b = stg.stream.bytes;
        CHECK(b[0] == 0x01 && b[1] == 0x00 && b[2] == 0x00 && b[3] == 0x02);  /* 0x02000001 */
        CHECK(b[4] == 0x09 && b[5] == 0 && b[6] == 0 && b[7] == 0);           /* flags */
        CHECK(b[8] == OLEUPDATE_ALWAYS && b[9] == 0 && b[10] == 0 && b[11] == 0);
        for (i = 12; i < 24; i++) {
            CHECK(b[i] == 0);       /* reserved, and the two moniker streams not written */
        }
        /* The size of the absolute source moniker stream counts itself:
           four, the class id's sixteen, and the moniker's own. */
        CHECK(b[24] == 20 + 4 && b[25] == 0 && b[26] == 0 && b[27] == 0);
        CHECK(memcmp(b + 28, &clsid_filemoniker, 16) == 0);
        CHECK(b[44] == 0xa0 && b[45] == 0xa1 && b[46] == 0xa2 && b[47] == 0xa3);
        /* The tail: a class id indicator with no class, an empty
           display name, and four times nothing. */
        CHECK(b[48] == 0xff && b[49] == 0xff && b[50] == 0xff && b[51] == 0xff);
        for (i = 52; i < 72; i++) {
            CHECK(b[i] == 0);
        }
        CHECK(b[72] == 0xff && b[73] == 0xff && b[74] == 0xff && b[75] == 0xff);
        for (i = 76; i < 100; i++) {
            CHECK(b[i] == 0);
        }

        /* Saving is not finishing: the link stays dirty until it is
           told the save went through, and only then is it clean.
           Handing the storage back and forth afterwards is harmless. */
        CHECK(IPersistStorage_IsDirty(p) == S_OK);
        CHECK(IPersistStorage_SaveCompleted(p, NULL) == S_OK);
        CHECK(IPersistStorage_IsDirty(p) == S_FALSE);
        CHECK(IPersistStorage_HandsOffStorage(p) == S_OK);
        CHECK(IPersistStorage_SaveCompleted(p, (IStorage *)&stg) == S_OK);
        CHECK(IPersistStorage_IsDirty(p) == S_FALSE);

        IPersistStorage_Release(p);
        IUnknown_Release(link);
    }

    /* A moniker whose Save refuses: that error, and a stream left
       truncated with the size it never learnt still zero. */
    {
        IPersistStorage *p;
        link = a_link();
        mk.save_hr = STG_E_WRITEFAULT_;
        p = link_as(link, &iid_persiststorage);
        CHECK(IPersistStorage_Save(p, (IStorage *)&stg, TRUE) == STG_E_WRITEFAULT_);
        CHECK(stg.stream.len == 44);
        CHECK(stg.stream.bytes[24] == 0);
        IPersistStorage_Release(p);
        IUnknown_Release(link);
    }

    /* A storage that will not make the stream: that error, and
       nothing written. */
    {
        IPersistStorage *p;
        link = a_link();
        stg.createstream_hr = STG_E_ACCESSDENIED_;
        p = link_as(link, &iid_persiststorage);
        CHECK(IPersistStorage_Save(p, (IStorage *)&stg, TRUE) == STG_E_ACCESSDENIED_);
        CHECK(mk.saves == 0);
        IPersistStorage_Release(p);
        IUnknown_Release(link);
    }

    /* ---- IDataObject: everything needs a running source, except the
           advise holder, which is the link's own ---- */
    {
        IDataObject *d;
        FORMATETC fe;
        FORMATETC canonical;
        STGMEDIUM medium;
        IEnumFORMATETC *formats_out = (IEnumFORMATETC *)(ULONG_PTR)0xdeadbeef;
        IEnumSTATDATA *advises = NULL;
        DWORD conn = 0xcccccccc;

        link = a_link();
        d = link_as(link, &iid_dataobject);
        a_format(&fe, CF_METAFILEPICT);
        ZeroMemory(&medium, sizeof medium);

        CHECK(IDataObject_QueryGetData(d, &fe) == OLE_E_NOTRUNNING_);
        CHECK(IDataObject_GetData(d, &fe, &medium) == OLE_E_NOTRUNNING_);
        CHECK(IDataObject_GetDataHere(d, &fe, &medium) == OLE_E_NOTRUNNING_);
        CHECK(IDataObject_GetCanonicalFormatEtc(d, &fe, &canonical) == OLE_E_NOTRUNNING_);
        CHECK(IDataObject_SetData(d, &fe, &medium, FALSE) == OLE_E_NOTRUNNING_);
        CHECK(IDataObject_EnumFormatEtc(d, DATADIR_GET, &formats_out) == REGDB_E_CLASSNOTREG_);
        CHECK(formats_out == NULL);

        CHECK(IDataObject_DAdvise(d, &fe, 0, (IAdviseSink *)&sink, &conn) == S_OK);
        CHECK(conn == 1);
        CHECK(IDataObject_DUnadvise(d, conn) == S_OK);
        CHECK(IDataObject_EnumDAdvise(d, &advises) == S_OK);
        if (advises) {
            IEnumSTATDATA_Release(advises);
        }

        IDataObject_Release(d);
        IUnknown_Release(link);
    }

    /* ---- IViewObject2 ---- */
    {
        IViewObject2 *v;
        SIZEL size;
        DWORD frozen = 0xcccccccc;
        DWORD aspects = 0xcccccccc;
        DWORD advise_flags = 0xcccccccc;
        IAdviseSink *got = NULL;

        link = a_link();
        v = link_as(link, &iid_viewobject2);

        /* Blank, and the caller's size is left as it was. */
        size.cx = -1;
        size.cy = -1;
        CHECK(IViewObject2_GetExtent(v, DVASPECT_CONTENT, -1, NULL, &size) == OLE_E_BLANK_);
        CHECK(size.cx == -1 && size.cy == -1);
        CHECK(IViewObject2_Freeze(v, DVASPECT_CONTENT, -1, NULL, &frozen) == OLE_E_BLANK_);
        CHECK(IViewObject2_Unfreeze(v, 0) == OLE_E_NOCONNECTION_);

        /* The view advise is remembered and read back whole. */
        CHECK(IViewObject2_SetAdvise(v, DVASPECT_CONTENT, 0, (IAdviseSink *)&sink) == S_OK);
        CHECK(IViewObject2_GetAdvise(v, &aspects, &advise_flags, &got) == S_OK);
        CHECK(aspects == DVASPECT_CONTENT);
        CHECK(advise_flags == 0);
        CHECK(got == (IAdviseSink *)&sink);
        if (got) {
            IAdviseSink_Release(got);
        }
        CHECK(IViewObject2_SetAdvise(v, DVASPECT_CONTENT, 0, NULL) == S_OK);

        IViewObject2_Release(v);
        IUnknown_Release(link);
    }

    /* ---- IOleCache2 ---- */
    {
        IOleCache2 *c;
        FORMATETC fe;
        IEnumSTATDATA *cached = NULL;
        DWORD conn = 0xcccccccc;

        link = a_link();
        c = link_as(link, &iid_olecache2);
        a_format(&fe, CF_METAFILEPICT);

        /* A presentation can be asked for and taken back without a
           source; what it would hold cannot be fetched. */
        CHECK(IOleCache2_Cache(c, &fe, 0, &conn) == S_OK);
        CHECK(conn != 0);
        CHECK(IOleCache2_Uncache(c, conn) == S_OK);
        CHECK(IOleCache2_EnumCache(c, &cached) == S_OK);
        if (cached) {
            STATDATA one;
            ULONG got = 0;
            CHECK(IEnumSTATDATA_Next(cached, 1, &one, &got) == S_FALSE);
            IEnumSTATDATA_Release(cached);
        }
        CHECK(IOleCache2_InitCache(c, NULL) == E_INVALIDARG);
        CHECK(IOleCache2_UpdateCache(c, NULL, UPDFCACHE_ALL, NULL) == OLE_E_NOTRUNNING_);
        CHECK(IOleCache2_DiscardCache(c, DISCARDCACHE_NOSAVE) == S_OK);

        IOleCache2_Release(c);
        IUnknown_Release(link);
    }

    /* Nothing is cached between calls: two links on one storage are
       two objects. */
    {
        void *second = NULL;
        begin();
        hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL,
                             NULL, NULL, NULL, (IStorage *)&stg, &out);
        CHECK(hr == S_OK);
        hr = OleCreateLinkEx((IMoniker *)&mk, &iid_unknown, 0, OLERENDER_NONE, 0, NULL, NULL,
                             NULL, NULL, NULL, (IStorage *)&stg, &second);
        CHECK(hr == S_OK);
        CHECK(second != out);
        CHECK(stg.classes == 2);
        CHECK(mk.refs == 3);
        IUnknown_Release((IUnknown *)out);
        IUnknown_Release((IUnknown *)second);
        CHECK(mk.refs == 1);
        CHECK(stg.refs == 1);
    }

    (void)conns;
    OleUninitialize();
    printf("olelink_test: ok (%d checks)\n", check);
    fflush(stdout);
    return 0;
}
