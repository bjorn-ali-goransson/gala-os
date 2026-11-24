/* ole32 as real Windows behaves, for what programs import and gala lacked
 * (docs/ROADMAP.md AG): wxWidgets (FileZilla, FileZilla Server's GUI,
 * pgAdmin's StackBuilder) imports PropVariantClear, ReleaseStgMedium,
 * OleRun, OleLockRunning, OleSetContainedObject; SQL Server LocalDB
 * imports CoSetProxyBlanket, CoQueryProxyBlanket and
 * CoFreeUnusedLibraries.
 *
 * Expected values are from Microsoft's documentation (learn.microsoft.com:
 * PropVariantClear, ReleaseStgMedium, OleRun, OleLockRunning,
 * OleSetContainedObject, CoSetProxyBlanket) and Wine's conformance tests
 * (dlls/ole32/tests/propvariant.c test_validtypes, validated on Windows).
 * The program is run on Windows 11 itself, which is what the answers
 * below are, and where five of them turned out not to be what the
 * documentation and Wine's tests say: revoking a bind-context object
 * parameter that is not registered is E_FAIL and not S_FALSE,
 * CreateBindCtx takes a reserved 1 as well as 0, PropVariantCopy
 * answers an invalid type with DISP_E_BADVARTYPE where
 * PropVariantClear gives STG_E_INVALIDPARAMETER, a stream whose
 * block was freed under it faults (STG_E_READFAULT,
 * STG_E_WRITEFAULT) instead of reading as empty, and CoFileTimeNow
 * comes at a SYSTEMTIME's resolution and so can sit behind a
 * GetSystemTimeAsFileTime taken just before it. In short:
 *
 * - PropVariantClear frees what the PROPVARIANT owns (strings with the
 *   task allocator or SysFreeString, interfaces released, vectors and
 *   their elements, arrays destroyed; by-reference values are not
 *   owned) and zeroes it; an invalid type (0x5555, a vector of
 *   VT_DISPATCH) gives STG_E_INVALIDPARAMETER and is zeroed too.
 * - ReleaseStgMedium: TYMED_HGLOBAL freed, TYMED_ISTREAM released,
 *   TYMED_FILE deleted and its name freed; with pUnkForRelease only that
 *   is released.
 * - OleRun / OleLockRunning / OleSetContainedObject call the object's
 *   IRunnableObject (Run, LockRunning, SetContainedObject) and return its
 *   result; an object without one is S_OK.
 * - CoSetProxyBlanket / CoQueryProxyBlanket on an object that is not a
 *   proxy (no IClientSecurity): E_NOINTERFACE.
 * - CoCreateInstanceEx: each MULTI_QI's hr and pItf; S_OK when all were
 *   found, CO_S_NOTALLINTERFACES when some, the activation's error (each
 *   hr too) for an unregistered class, E_INVALIDARG for none asked or a
 *   pItf that is not NULL on input (the entries before it reset to
 *   E_NOINTERFACE, it and those after untouched).
 * - CreateBindCtx: a bind context whose default BIND_OPTS are grfMode
 *   STGM_READWRITE and the rest 0; options round-trip; bound objects
 *   are held (AddRef) until revoked (MK_E_NOTBOUND after) or
 *   ReleaseBoundObjects; object parameters by name (E_FAIL for an
 *   unknown one, and E_FAIL revoking one that is not there -- not the
 *   documented S_FALSE) until revoked or the context ends; a reserved
 *   reserved value of 0 or 1 is taken, 2 and above are E_INVALIDARG
 *   and leave the caller's pointer untouched.
 * - CoImpersonateClient / CoRevertToSelf outside a COM call:
 *   RPC_E_CALL_COMPLETE (there is no call context).
 *
 * - HSTRING (winstring.h, through api-ms-win-core-winrt-string, as
 *   Chromium imports it): WindowsCreateString copies (NULL for length 0,
 *   E_POINTER for a NULL source with a length); a reference string
 *   (WindowsCreateStringReference) uses the caller's text, which must
 *   be NUL-terminated at its length (a failure: Windows gives
 *   0x80000017, Wine's string tests note, the documentation
 *   E_INVALIDARG; combase_test has the rest); raw buffers, lengths
 *   (L"" and 0 for NULL), duplicates (a copy of a reference string),
 *   ordinal comparison; deleting NULL or a reference string is S_OK.
 * - RoGetActivationFactory / RoActivateInstance of a class that is not
 *   registered: REGDB_E_CLASSNOTREG, the output NULL.
 * - CoMarshalInterThreadInterfaceInStream + CoGetInterfaceAndReleaseStream
 *   in the object's own apartment: the object itself comes back; the
 *   stream holds one reference until unmarshaled. A TABLESTRONG marshal
 *   unmarshals twice and holds its reference until CoReleaseMarshalData.
 * - RoGetAgileReference: Resolve gives the object; E_NOINTERFACE for an
 *   interface it lacks; the reference is released with the agile one.
 * - CoRegisterMessageFilter outside an STA: CO_E_NOT_SUPPORTED.
 * - CoRegisterActivationFilter (Word's MSO.DLL delay-loads it, which is
 *   where Office stopped): the filter belongs to the process, not to an
 *   apartment, so it registers before CoInitializeEx and outlives
 *   CoUninitialize; nothing asks it for an interface or holds a
 *   reference on it; the first filter wins (the same pointer again S_OK,
 *   a different one CO_E_NOT_SUPPORTED) and a NULL filter is
 *   E_INVALIDARG and removes nothing. It is consulted on every
 *   activation, CoGetClassObject as well as CoCreateInstance, with the
 *   activation type 0 and a replacement CLSID that is GUID_NULL on
 *   entry: a filter that names another class has that one activated and
 *   is asked about it too, and a filter that fails turns the activation
 *   into E_ACCESSDENIED whatever it returned. How many times it is
 *   consulted for one CoCreateInstance is not fixed -- activating a
 *   class can activate others, and the filter is asked about those as
 *   well (CUIAutomation brings two with it on Windows) -- so what is
 *   checked is the order the classes come in. ole32's entry and
 *   combase's are the same function.
 * - PropVariantCopy: a VT_LPWSTR is copied (a new string), a VT_UNKNOWN
 *   AddRef'd, a vector of strings copied element by element; an invalid
 *   type is DISP_E_BADVARTYPE -- not the STG_E_INVALIDPARAMETER
 *   PropVariantClear answers with for the very same type.
 * - CreateStreamOnHGlobal with an HGLOBAL (Wine's
 *   dlls/ole32/tests/hglobalstream.c): the stream starts with the
 *   block's contents and GlobalSize; writes past the end grow the block
 *   (GlobalReAlloc, so a GMEM_FIXED block moves and the stream's handle
 *   changes); GetHGlobalFromStream gives the handle (E_INVALIDARG for a
 *   NULL argument or another stream); clones share the block and size
 *   but have their own position, starting at the original's; the block
 *   is freed with the last stream when fDeleteOnRelease. A block freed
 *   under the stream keeps the handle and size it cached, and every
 *   call that touches the memory faults: STG_E_READFAULT and
 *   STG_E_WRITEFAULT, while SetSize and a write past the cached end
 *   are E_OUTOFMEMORY.
 *   Seek takes the low 32 bits of the move as signed and keeps the
 *   position in 32 bits: a result below 0 or past 4 GiB is
 *   STG_E_SEEKERROR (so is an unknown origin), which still reports the
 *   position; SetSize ignores the high 32 bits. CopyTo reads from this
 *   stream and writes what it read to the other.
 * - CoFileTimeNow: the system time as a FILETIME at a SYSTEMTIME's
 *   resolution -- truncated to whole milliseconds, so always a
 *   multiple of 10000 and able to sit a millisecond behind a
 *   GetSystemTimeAsFileTime taken just before the call.
 *   CoGetCurrentProcess: non-zero, the same on each call in a thread,
 *   another number in another thread.
 *
 * - A server process's lifetime (Delphi's ComServer looks these up; Inno
 *   Setup): CoAddRefServerProcess returns the new count (1, 2),
 *   CoReleaseServerProcess the count after it (1, 0);
 *   CoSuspendClassObjects and CoResumeClassObjects return S_OK.
 *
 * Prints "ole32_test: ok" or exits with the number of the failing check. */
#define COBJMACROS
#include <windows.h>
#include <ole2.h>
#include <oleauto.h>
#include <propidl.h>
#include <roapi.h>
#include <winstring.h>
#include <stdio.h>

static int check;
#define CHECK(cond) do { check++; if (!(cond)) { printf("ole32_test: check %d (line %d) failed\n", check, __LINE__); fflush(stdout); ExitProcess(check); } } while (0)

/* A COM object counting its references, optionally an IRunnableObject. */
typedef struct {
    IRunnableObjectVtbl *vtbl;
    LONG refs;
    BOOL runnable;
    int ran, locked, lock_closes, contained;
} Obj;

static HRESULT WINAPI obj_qi(IRunnableObject *this, REFIID iid, void **out) {
    Obj *o = (Obj *)this;
    if (IsEqualIID(iid, &IID_IUnknown) || (o->runnable && IsEqualIID(iid, &IID_IRunnableObject))) {
        *out = this;
        o->refs++;
        return S_OK;
    }
    *out = NULL;
    return E_NOINTERFACE;
}
static ULONG WINAPI obj_addref(IRunnableObject *this) { return ++((Obj *)this)->refs; }
static ULONG WINAPI obj_release(IRunnableObject *this) { return --((Obj *)this)->refs; }
static HRESULT WINAPI obj_class(IRunnableObject *this, CLSID *c) { (void)this; (void)c; return E_NOTIMPL; }
static HRESULT WINAPI obj_run(IRunnableObject *this, IBindCtx *ctx) {
    ((Obj *)this)->ran++;
    return ctx == NULL ? 0x00040123 : E_FAIL; /* a recognizable success */
}
static BOOL WINAPI obj_isrunning(IRunnableObject *this) { (void)this; return TRUE; }
static HRESULT WINAPI obj_lock(IRunnableObject *this, BOOL lock, BOOL closes) {
    Obj *o = (Obj *)this;
    o->locked = lock ? 1 : -1;
    o->lock_closes = closes;
    return S_FALSE;
}
static HRESULT WINAPI obj_contained(IRunnableObject *this, BOOL contained) {
    ((Obj *)this)->contained = contained ? 1 : -1;
    return S_OK;
}
static IRunnableObjectVtbl vtbl = {obj_qi, obj_addref, obj_release, obj_class, obj_run,
                                   obj_isrunning, obj_lock, obj_contained};

static void make(Obj *o, BOOL runnable) {
    memset(o, 0, sizeof *o);
    o->vtbl = &vtbl;
    o->refs = 1;
    o->runnable = runnable;
}

static int zeroed(const PROPVARIANT *pv) {
    return pv->vt == VT_EMPTY && pv->uhVal.QuadPart == 0;
}

static WCHAR *task_string(const WCHAR *s) {
    WCHAR *p = CoTaskMemAlloc((wcslen(s) + 1) * sizeof(WCHAR));
    wcscpy(p, s);
    return p;
}

/* The stream's handle, size (Stat) and position. */
static void stream_info(IStream *s, HGLOBAL *h, ULONG *size, ULONG *pos) {
    STATSTG st;
    LARGE_INTEGER zero = {{0, 0}};
    ULARGE_INTEGER at;
    *h = NULL;
    CHECK(GetHGlobalFromStream(s, h) == S_OK);
    memset(&st, 0x55, sizeof st);
    CHECK(IStream_Stat(s, &st, STATFLAG_DEFAULT) == S_OK && st.type == STGTY_STREAM);
    CHECK(st.pwcsName == NULL && st.cbSize.HighPart == 0);
    *size = st.cbSize.LowPart;
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_CUR, &at) == S_OK && at.HighPart == 0);
    *pos = at.LowPart;
}

static DWORD WINAPI other_thread(void *out) {
    *(DWORD *)out = CoGetCurrentProcess();
    return 0;
}

static void time_and_numbers(void) {
    FILETIME before, now, after;
    GetSystemTimeAsFileTime(&before);
    CHECK(CoFileTimeNow(&now) == S_OK);
    GetSystemTimeAsFileTime(&after);
    ULONGLONG b = ((ULONGLONG)before.dwHighDateTime << 32) | before.dwLowDateTime;
    ULONGLONG n = ((ULONGLONG)now.dwHighDateTime << 32) | now.dwLowDateTime;
    ULONGLONG a = ((ULONGLONG)after.dwHighDateTime << 32) | after.dwLowDateTime;
    /* The time comes at a SYSTEMTIME's resolution: whole milliseconds,
       the system time truncated rather than rounded, so it is a multiple
       of 10000 and sits between the reading before the call truncated the
       same way and the reading after it -- never between the two
       untruncated readings, which is what a program comparing its stamp
       against a finer clock relies on. */
    CHECK(n % 10000 == 0);
    CHECK(b - b % 10000 <= n && n <= a);
    DWORD mine = CoGetCurrentProcess(), theirs = 0;
    CHECK(mine != 0 && CoGetCurrentProcess() == mine);
    HANDLE t = CreateThread(NULL, 0, other_thread, &theirs, 0, NULL);
    CHECK(t != NULL && WaitForSingleObject(t, 10000) == WAIT_OBJECT_0);
    CloseHandle(t);
    CHECK(theirs != 0 && theirs != mine);
}

/* ---- OLE persistence and the class objects a process answers for:
   WriteClassStm, ReadClassStm, OleSaveToStream, OleLoadFromStream,
   CoRegisterClassObject and CoRevokeClassObject, which Word asks ole32
   for (ROADMAP OF). Almost nothing below is documented: the order of
   the calls each one makes on the object, where it leaves the stream,
   what it does to the reference counts, which of two registrations of
   one class serves, and what the contexts mean are all what this
   machine's Windows 11 answered.

   The one surprise is the context: a registration is found by an
   activation that names an in-process context and by no other, whatever
   context the registration itself named. A class registered for
   CLSCTX_LOCAL_SERVER is handed to a CLSCTX_INPROC_SERVER activation,
   and one registered for CLSCTX_INPROC_SERVER is not handed to a
   CLSCTX_LOCAL_SERVER one -- the table belongs to the process, and a
   local activation goes to the service control manager instead of
   reading it. */

/* The answer a process the service control manager did not start as a
   server gets from CoResumeClassObjects, which is every process here:
   Win32 error 1734 as an HRESULT ("the array bounds are invalid", which
   has nothing to do with anything). The suspension stands. */
#define CO_E_RESUME_NOT_A_SERVER ((HRESULT)0x800706C6L)

/* An object implementing IPersistStream, recording what it was asked
   and in which order, and answering what the case wants it to. */
enum { P_CLASS = 1, P_DIRTY, P_LOAD, P_SAVE, P_SIZEMAX, P_QI_PERSIST, P_QI_OTHER };

typedef struct {
    IPersistStreamVtbl *vtbl;
    LONG refs;
    CLSID id;
    HRESULT class_hr, save_hr, load_hr;
    unsigned long long seq;   /* the calls, 4 bits each, oldest lowest */
    int ncalls;
    IStream *saw;             /* the stream Save or Load was given */
    BOOL saw_clear;           /* fClearDirty */
    ULONG at;                 /* the position the stream was at then */
    int persist;              /* answers QI for IPersistStream */
} Pers;

static void pers_said(Pers *p, int what) {
    if (p->ncalls < 16) {
        p->seq |= (unsigned long long)what << (4 * p->ncalls);
    }
    p->ncalls++;
}

/* The calls as `seq` holds them: oldest in the lowest nibble. */
#define SEQ1(a) ((unsigned long long)(a))
#define SEQ2(a, b) (SEQ1(a) | ((unsigned long long)(b) << 4))
#define SEQ3(a, b, c) (SEQ2(a, b) | ((unsigned long long)(c) << 8))
#define SEQ4(a, b, c, d) (SEQ3(a, b, c) | ((unsigned long long)(d) << 12))

static ULONG stream_at(IStream *s) {
    LARGE_INTEGER zero = {{0, 0}};
    ULARGE_INTEGER at;
    at.QuadPart = 0;
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_CUR, &at) == S_OK);
    return (ULONG)at.QuadPart;
}

static ULONG stream_size(IStream *s) {
    STATSTG st;
    memset(&st, 0, sizeof st);
    CHECK(IStream_Stat(s, &st, STATFLAG_NONAME) == S_OK);
    return (ULONG)st.cbSize.QuadPart;
}

static HRESULT WINAPI pers_qi(IPersistStream *this, REFIID iid, void **out) {
    Pers *p = (Pers *)this;
    if (IsEqualIID(iid, &IID_IUnknown)
        || (p->persist
            && (IsEqualIID(iid, &IID_IPersistStream) || IsEqualIID(iid, &IID_IPersist)))) {
        pers_said(p, P_QI_PERSIST);
        *out = this;
        p->refs++;
        return S_OK;
    }
    pers_said(p, P_QI_OTHER);
    *out = NULL;
    return E_NOINTERFACE;
}
static ULONG WINAPI pers_addref(IPersistStream *this) { return ++((Pers *)this)->refs; }
static ULONG WINAPI pers_release(IPersistStream *this) { return --((Pers *)this)->refs; }
static HRESULT WINAPI pers_class(IPersistStream *this, CLSID *id) {
    Pers *p = (Pers *)this;
    pers_said(p, P_CLASS);
    if (p->class_hr != S_OK) {
        return p->class_hr;
    }
    *id = p->id;
    return S_OK;
}
static HRESULT WINAPI pers_dirty(IPersistStream *this) {
    pers_said((Pers *)this, P_DIRTY);
    return S_OK;
}
static HRESULT WINAPI pers_load(IPersistStream *this, IStream *s) {
    Pers *p = (Pers *)this;
    pers_said(p, P_LOAD);
    p->saw = s;
    p->at = stream_at(s);
    return p->load_hr;
}
static HRESULT WINAPI pers_save(IPersistStream *this, IStream *s, BOOL clear) {
    Pers *p = (Pers *)this;
    ULONG n = 0;
    pers_said(p, P_SAVE);
    p->saw = s;
    p->saw_clear = clear;
    p->at = stream_at(s);
    if (p->save_hr != S_OK) {
        return p->save_hr;
    }
    CHECK(IStream_Write(s, "SAVE", 4, &n) == S_OK && n == 4);
    return S_OK;
}
static HRESULT WINAPI pers_sizemax(IPersistStream *this, ULARGE_INTEGER *size) {
    pers_said((Pers *)this, P_SIZEMAX);
    size->QuadPart = 64;
    return S_OK;
}
static IPersistStreamVtbl pers_vtbl = {pers_qi,    pers_addref, pers_release, pers_class,
                                       pers_dirty, pers_load,   pers_save,    pers_sizemax};

static const CLSID clsid_test = {0x11223344, 0x5566, 0x7788,
                                 {0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01}};
static const CLSID clsid_none = {0x9e5d1f4b, 0x1c3a, 0x4ab7,
                                 {0x8f, 0x2d, 0x6b, 0x44, 0x19, 0x7c, 0x05, 0xe3}};
/* CLSID_CUIAutomation, which is registered and has no IPersistStream,
   on Windows and on gala alike (user/uiautomationcore). */
static const CLSID clsid_uia = {0xff48dba4, 0x60ef, 0x4201,
                                {0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e}};

static void pers_make(Pers *p, int persist) {
    memset(p, 0, sizeof *p);
    p->vtbl = &pers_vtbl;
    p->refs = 1;
    p->id = clsid_test;
    p->persist = persist;
}

static BOOL clsid_is_zero(const CLSID *id) {
    CLSID zero;
    memset(&zero, 0, sizeof zero);
    return memcmp(id, &zero, sizeof zero) == 0;
}

/* A stream holding one CLSID and nothing else, rewound. */
static IStream *stream_of_class(const CLSID *id) {
    IStream *s;
    LARGE_INTEGER zero = {{0, 0}};
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    if (id != NULL) {
        CHECK(WriteClassStm(s, id) == S_OK);
    }
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, NULL) == S_OK);
    return s;
}

/* A class factory handing out Pers objects, for CoRegisterClassObject
   and so for the whole of OleLoadFromStream. */
typedef struct {
    IClassFactoryVtbl *vtbl;
    LONG refs;
    int made, locked;
    IID asked;
    Pers *give;
    HRESULT make_hr;
} Fact;

static HRESULT WINAPI fact_qi(IClassFactory *this, REFIID iid, void **out) {
    Fact *f = (Fact *)this;
    if (IsEqualIID(iid, &IID_IUnknown) || IsEqualIID(iid, &IID_IClassFactory)) {
        *out = this;
        f->refs++;
        return S_OK;
    }
    *out = NULL;
    return E_NOINTERFACE;
}
static ULONG WINAPI fact_addref(IClassFactory *this) { return ++((Fact *)this)->refs; }
static ULONG WINAPI fact_release(IClassFactory *this) { return --((Fact *)this)->refs; }
static HRESULT WINAPI fact_make(IClassFactory *this, IUnknown *outer, REFIID iid, void **out) {
    Fact *f = (Fact *)this;
    f->made++;
    f->asked = *iid;
    if (f->make_hr != S_OK) {
        *out = NULL;
        return f->make_hr;
    }
    if (outer != NULL) {
        *out = NULL;
        return CLASS_E_NOAGGREGATION;
    }
    return pers_qi((IPersistStream *)f->give, iid, out);
}
static HRESULT WINAPI fact_lock(IClassFactory *this, BOOL lock) {
    ((Fact *)this)->locked += lock ? 1 : -1;
    return S_OK;
}
static IClassFactoryVtbl fact_vtbl = {fact_qi, fact_addref, fact_release, fact_make, fact_lock};

static void fact_make_it(Fact *f, Pers *gives) {
    memset(f, 0, sizeof *f);
    f->vtbl = &fact_vtbl;
    f->refs = 1;
    f->give = gives;
}

/* Makes one instance of clsid_test from whatever answers for it, and
   releases it: the count of a factory an activation found, and the
   activation's own result, are what the cases ask about. */
static HRESULT made_one(DWORD context, void **out) {
    void *obj = NULL;
    HRESULT hr = CoCreateInstance(&clsid_test, NULL, context, &IID_IPersistStream, &obj);
    if (out != NULL) {
        *out = obj;
    }
    if (obj != NULL) {
        IUnknown_Release((IUnknown *)obj);
    }
    return hr;
}

static void persist_streams(void) {
    IStream *s;
    LARGE_INTEGER zero = {{0, 0}}, eight = {{8, 0}};
    CLSID got, null_id;
    ULONG n;
    char buf[32];
    Pers p;
    void *out;

    memset(&null_id, 0, sizeof null_id);

    /* WriteClassStm writes the sixteen bytes of the CLSID as it sits in
       memory, at the stream's position, and leaves the stream after
       them. */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    CHECK(WriteClassStm(s, &clsid_test) == S_OK);
    CHECK(stream_at(s) == 16 && stream_size(s) == 16);
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, NULL) == S_OK);
    memset(buf, 0x55, sizeof buf);
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 16);
    CHECK(memcmp(buf, &clsid_test, 16) == 0);

    /* ReadClassStm reads it back. At the end of the stream, and with
       half a CLSID left, it is STG_E_READFAULT with the caller's CLSID
       zeroed and the stream left where the read stopped -- at the end
       in both cases, so a short read has consumed what there was. */
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, NULL) == S_OK);
    memset(&got, 0x55, sizeof got);
    CHECK(ReadClassStm(s, &got) == S_OK && IsEqualCLSID(&got, &clsid_test));
    CHECK(stream_at(s) == 16);
    memset(&got, 0x55, sizeof got);
    CHECK(ReadClassStm(s, &got) == STG_E_READFAULT && clsid_is_zero(&got));
    CHECK(stream_at(s) == 16);
    CHECK(IStream_Seek(s, eight, STREAM_SEEK_SET, NULL) == S_OK);
    memset(&got, 0x55, sizeof got);
    CHECK(ReadClassStm(s, &got) == STG_E_READFAULT && clsid_is_zero(&got));
    CHECK(stream_at(s) == 16);
    IStream_Release(s);

    /* OleSaveToStream asks the object for its class id, writes that,
       and calls Save(stream, TRUE): two calls and no others. It does
       not ask IsDirty, does not ask GetSizeMax, asks the object for no
       interface and holds no reference on it, and Save sees the stream
       after the class id. */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    pers_make(&p, 1);
    CHECK(OleSaveToStream((IPersistStream *)&p, s) == S_OK);
    CHECK(p.ncalls == 2 && p.seq == SEQ2(P_CLASS, P_SAVE));
    CHECK(p.saw == s && p.saw_clear == TRUE && p.at == 16);
    CHECK(p.refs == 1 && stream_at(s) == 20 && stream_size(s) == 20);
    CHECK(IStream_Seek(s, zero, STREAM_SEEK_SET, NULL) == S_OK);
    memset(buf, 0x55, sizeof buf);
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 20);
    CHECK(memcmp(buf, &clsid_test, 16) == 0 && memcmp(buf + 16, "SAVE", 4) == 0);
    IStream_Release(s);

    /* GetClassID refusing: that error, with nothing written and Save
       never called. */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    pers_make(&p, 1);
    p.class_hr = E_FAIL;
    CHECK(OleSaveToStream((IPersistStream *)&p, s) == E_FAIL);
    CHECK(p.ncalls == 1 && p.seq == SEQ1(P_CLASS) && stream_size(s) == 0);
    IStream_Release(s);

    /* Save refusing: that error, with the class id left in the stream
       (nothing is undone). */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    pers_make(&p, 1);
    p.save_hr = E_OUTOFMEMORY;
    CHECK(OleSaveToStream((IPersistStream *)&p, s) == E_OUTOFMEMORY);
    CHECK(p.seq == SEQ2(P_CLASS, P_SAVE) && stream_size(s) == 16);
    IStream_Release(s);

    /* A class id of CLSID_NULL is written and saved like any other. */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    pers_make(&p, 1);
    p.id = null_id;
    CHECK(OleSaveToStream((IPersistStream *)&p, s) == S_OK);
    CHECK(p.seq == SEQ2(P_CLASS, P_SAVE) && stream_size(s) == 20);
    IStream_Release(s);

    /* OleLoadFromStream reads the class id first: an empty stream is
       STG_E_READFAULT with the caller's pointer zeroed and the stream
       untouched. */
    s = stream_of_class(NULL);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IUnknown, &out) == STG_E_READFAULT);
    CHECK(out == NULL && stream_at(s) == 0);
    IStream_Release(s);

    /* CLSID_NULL and a class no machine has are both
       REGDB_E_CLASSNOTREG, and a class that is registered but has no
       IPersistStream is E_NOINTERFACE: each has read the class id, so
       the stream sits after it. */
    s = stream_of_class(&null_id);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IUnknown, &out) == REGDB_E_CLASSNOTREG);
    CHECK(out == NULL && stream_at(s) == 16);
    IStream_Release(s);

    s = stream_of_class(&clsid_none);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IUnknown, &out) == REGDB_E_CLASSNOTREG);
    CHECK(out == NULL && stream_at(s) == 16);
    IStream_Release(s);

    s = stream_of_class(&clsid_uia);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IUnknown, &out) == E_NOINTERFACE);
    CHECK(out == NULL && stream_at(s) == 16);
    IStream_Release(s);
}

/* CoRegisterClassObject, and the whole of OleLoadFromStream over a
   class this process answers for -- which is the only way to see the
   calls it makes, since every class in the registry here is one whose
   code we do not control. */
static void class_objects(void) {
    Fact f;
    Pers p;
    DWORD key = 0x5555, key2 = 0x5555;
    IStream *s;
    void *out;

    /* The registration takes one reference on the class object and
       hands back a key that is neither zero nor the caller's old
       value. */
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key)
          == S_OK);
    CHECK(key != 0 && key != 0x5555 && f.refs == 2);

    /* The class is activated from here now, and not from the registry
       (which has nothing for it). */
    out = NULL;
    CHECK(made_one(CLSCTX_INPROC_SERVER, &out) == S_OK);
    CHECK(out == (void *)&p && f.made == 1);

    /* The same class may be registered a second time, for another key
       and another reference. */
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key2)
          == S_OK);
    CHECK(key2 != key && f.refs == 3);
    CHECK(CoRevokeClassObject(key2) == S_OK && f.refs == 2);

    /* And a registration beats the registry: CUIAutomation is
       registered on this machine, and this process answers for it. */
    CHECK(CoRegisterClassObject(&clsid_uia, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key2)
          == S_OK);
    out = NULL;
    CHECK(CoCreateInstance(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, &IID_IUnknown, &out) == S_OK);
    CHECK(out == (void *)&p);
    IUnknown_Release((IUnknown *)out);
    CHECK(CoRevokeClassObject(key2) == S_OK);

    /* The whole of OleLoadFromStream, in order: the factory is asked to
       make the caller's own interface, the object is then asked for
       IPersistStream, Load is given the stream after the class id, and
       the caller's interface is asked for once more at the end. Four
       calls; the object is left holding the reference it was made with
       and the caller's. */
    p.seq = 0;
    p.ncalls = 0;
    p.saw = NULL;
    p.at = 0;
    f.made = 0;
    s = stream_of_class(&clsid_test);
    out = NULL;
    CHECK(OleLoadFromStream(s, &IID_IPersistStream, &out) == S_OK);
    CHECK(out == (void *)&p && f.made == 1);
    CHECK(p.ncalls == 4 && p.seq == SEQ4(P_QI_PERSIST, P_QI_PERSIST, P_LOAD, P_QI_PERSIST));
    CHECK(p.saw == s && p.at == 16 && stream_at(s) == 16);
    CHECK(p.refs == 2);
    IUnknown_Release((IUnknown *)out);
    IStream_Release(s);

    /* An interface the object does not have: the one asking is the
       factory's, so Load never runs -- but the class id has been read,
       which is what says the refusal came after it. */
    pers_make(&p, 1);
    f.made = 0;
    s = stream_of_class(&clsid_test);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IDataObject, &out) == E_NOINTERFACE);
    CHECK(out == NULL && p.ncalls == 1 && p.seq == SEQ1(P_QI_OTHER));
    CHECK(p.at == 0 && stream_at(s) == 16 && p.refs == 1);
    IStream_Release(s);

    /* Load refusing: its own code, the caller's pointer zeroed and the
       object released. */
    pers_make(&p, 1);
    p.load_hr = STG_E_INVALIDHEADER;
    s = stream_of_class(&clsid_test);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IPersistStream, &out) == STG_E_INVALIDHEADER);
    CHECK(out == NULL && p.refs == 1);
    IStream_Release(s);

    /* The factory refusing to make one: its own code. */
    pers_make(&p, 1);
    f.make_hr = E_OUTOFMEMORY;
    s = stream_of_class(&clsid_test);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(OleLoadFromStream(s, &IID_IPersistStream, &out) == E_OUTOFMEMORY);
    CHECK(out == NULL && stream_at(s) == 16);
    IStream_Release(s);
    f.make_hr = S_OK;

    /* Revoking gives the reference back and the class to the registry
       again. The key is then a stale one, and revoking a stale or an
       unknown key is S_OK. */
    CHECK(CoRevokeClassObject(key) == S_OK && f.refs == 1);
    out = (void *)(ULONG_PTR)0x5555;
    CHECK(made_one(CLSCTX_INPROC_SERVER, &out) == REGDB_E_CLASSNOTREG && out == NULL);
    CHECK(CoRevokeClassObject(key) == S_OK);
    CHECK(CoRevokeClassObject(0x1234) == S_OK);
}

/* What a registration does to the count, which of two registrations of
   one class serves, what REGCLS_SINGLEUSE leaves behind, and what
   suspending does. */
static void class_object_counts(void) {
    Fact f, g;
    Pers p, q;
    DWORD key = 0, key2 = 0, k = 0;
    int before;

    pers_make(&p, 1);
    pers_make(&q, 1);
    fact_make_it(&f, &p);
    fact_make_it(&g, &q);

    /* One reference for the registration, given back by the revoke, and
       an activation that leaves the count where it found it (the
       factory is handed out with a reference and released). */
    CHECK(f.refs == 1);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key)
          == S_OK);
    CHECK(f.refs == 2);
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == S_OK && f.refs == 2);
    CHECK(CoRevokeClassObject(key) == S_OK && f.refs == 1);

    /* Two registrations of one class: the newer serves, and the older
       serves again once the newer is revoked. */
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key)
          == S_OK);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&g, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key2)
          == S_OK);
    f.made = 0;
    g.made = 0;
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == S_OK);
    CHECK(f.made == 0 && g.made == 1);
    CHECK(CoRevokeClassObject(key2) == S_OK);
    f.made = 0;
    g.made = 0;
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == S_OK);
    CHECK(f.made == 1 && g.made == 0);
    CHECK(CoRevokeClassObject(key) == S_OK);

    /* REGCLS_SINGLEUSE serves once; the next activation is
       REGDB_E_CLASSNOTREG, and the registration is still holding its
       reference on the class object until it is revoked. */
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_SINGLEUSE, &k)
          == S_OK);
    before = f.refs;
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == S_OK);
    CHECK(f.refs == before && f.refs == 2);
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == REGDB_E_CLASSNOTREG);
    CHECK(CoRevokeClassObject(k) == S_OK && f.refs == 1);

    /* REGCLS_SUSPENDED registers and does not serve, and
       CoResumeClassObjects does not change that here: it is
       CO_E_RESUME_NOT_A_SERVER and the registration stays suspended.
       CoSuspendClassObjects is S_OK and suspends the rest. */
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED, &k)
          == S_OK);
    CHECK(CoResumeClassObjects() == CO_E_RESUME_NOT_A_SERVER);
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == REGDB_E_CLASSNOTREG);
    CHECK(CoSuspendClassObjects() == S_OK);
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == REGDB_E_CLASSNOTREG);
    CHECK(CoResumeClassObjects() == CO_E_RESUME_NOT_A_SERVER);
    CHECK(CoRevokeClassObject(k) == S_OK);

    /* The same with nothing suspended at all, and with a plain
       registration standing: the answer is that error either way. */
    CHECK(CoResumeClassObjects() == CO_E_RESUME_NOT_A_SERVER);
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &k)
          == S_OK);
    CHECK(CoResumeClassObjects() == CO_E_RESUME_NOT_A_SERVER);
    CHECK(CoRevokeClassObject(k) == S_OK);
}

/* The four that decide what an implementation has to keep: whether an
   apartment or a thread owns a registration, what the server process
   count reaching zero does to one, what an interface the class object
   does not have gives, and what the contexts mean. */
static HRESULT mta_hr;
static DWORD WINAPI mta_sees(void *unused) {
    HRESULT init = CoInitializeEx(NULL, COINIT_MULTITHREADED);
    (void)unused;
    mta_hr = made_one(CLSCTX_INPROC_SERVER, NULL);
    if (init == S_OK || init == S_FALSE) {
        CoUninitialize();
    }
    return 0;
}

static void class_object_more(void) {
    Fact f;
    Pers p;
    DWORD k = 0, k2 = 0x5555;
    HANDLE t;
    void *cf, *out;

    /* This thread is the MTA, and another MTA thread is in the same
       apartment: it finds the registration and the factory serves it.
       An apartment owns a registration, not a thread. */
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &k)
          == S_OK);
    f.made = 0;
    mta_hr = 0x5555;
    t = CreateThread(NULL, 0, mta_sees, NULL, 0, NULL);
    CHECK(t != NULL && WaitForSingleObject(t, 10000) == WAIT_OBJECT_0);
    CloseHandle(t);
    CHECK(mta_hr == S_OK && f.made == 1);

    /* An interface the class object does not have: E_NOINTERFACE with
       the caller's pointer zeroed, and nothing made. */
    f.made = 0;
    cf = (void *)(ULONG_PTR)0x5555;
    CHECK(CoGetClassObject(&clsid_test, CLSCTX_INPROC_SERVER, NULL, &IID_IPersistStream, &cf)
          == E_NOINTERFACE);
    CHECK(cf == NULL && f.made == 0);

    /* The server process count reaching zero suspends the class objects
       the service control manager knows about, which is none of these:
       this registration goes on serving, and the resume that the
       documentation pairs with it is the same error as ever. */
    CHECK(CoAddRefServerProcess() == 1);
    CHECK(CoReleaseServerProcess() == 0);
    out = NULL;
    CHECK(made_one(CLSCTX_INPROC_SERVER, &out) == S_OK && out == (void *)&p);
    CHECK(CoResumeClassObjects() == CO_E_RESUME_NOT_A_SERVER);
    out = NULL;
    CHECK(made_one(CLSCTX_INPROC_SERVER, &out) == S_OK && out == (void *)&p);
    CHECK(CoRevokeClassObject(k) == S_OK);

    /* A registration that named the local server is handed to an
       in-process activation all the same, and a local activation finds
       nothing: what decides is the context the activation asks for, not
       the one the registration named. */
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_LOCAL_SERVER,
                                REGCLS_MULTIPLEUSE, &k)
          == S_OK);
    out = NULL;
    CHECK(made_one(CLSCTX_INPROC_SERVER, &out) == S_OK && out == (void *)&p && f.made == 1);
    CHECK(made_one(CLSCTX_LOCAL_SERVER, NULL) == REGDB_E_CLASSNOTREG && f.made == 1);
    CHECK(CoRevokeClassObject(k) == S_OK);

    /* No class object at all: E_INVALIDARG, with the caller's key
       zeroed (the apartment was there, so it got that far). */
    CHECK(CoRegisterClassObject(&clsid_test, NULL, CLSCTX_INPROC_SERVER, REGCLS_MULTIPLEUSE, &k2)
          == E_INVALIDARG);
    CHECK(k2 == 0);
}

/* What CoRegisterClassObject refuses, what another apartment sees of a
   registration, and what CoUninitialize does to one. */
static DWORD WINAPI reg_from_thread(void *out) {
    HRESULT init = CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);
    *(HRESULT *)out = made_one(CLSCTX_INPROC_SERVER, NULL);
    if (init == S_OK || init == S_FALSE) {
        CoUninitialize();
    }
    return 0;
}

static void class_object_edges(void) {
    Fact f;
    Pers p;
    DWORD key = 0, k2;
    HRESULT other = 0;
    HANDLE t;
    void *cf;
    int before;

    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key)
          == S_OK);

    /* The factory is asked for the interface the caller asked for, and
       not for IUnknown. */
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == S_OK);
    CHECK(IsEqualIID(&f.asked, &IID_IPersistStream) && !IsEqualIID(&f.asked, &IID_IUnknown));

    /* CoGetClassObject of a class this process answers for hands back
       the class object itself, with one reference for the caller. */
    before = f.refs;
    cf = NULL;
    CHECK(CoGetClassObject(&clsid_test, CLSCTX_INPROC_SERVER, NULL, &IID_IClassFactory, &cf)
          == S_OK);
    CHECK(cf == (void *)&f && f.refs == before + 1);
    IClassFactory_Release((IClassFactory *)cf);

    /* A local activation does not read this table; CLSCTX_ALL does,
       because it names the in-process contexts too. */
    CHECK(made_one(CLSCTX_LOCAL_SERVER, NULL) == REGDB_E_CLASSNOTREG);
    CHECK(made_one(CLSCTX_ALL, NULL) == S_OK);

    /* Another apartment: a single-threaded one of its own sees nothing
       of a registration made in the MTA. */
    f.made = 0;
    t = CreateThread(NULL, 0, reg_from_thread, &other, 0, NULL);
    CHECK(t != NULL && WaitForSingleObject(t, 10000) == WAIT_OBJECT_0);
    CloseHandle(t);
    CHECK(other == REGDB_E_CLASSNOTREG && f.made == 0);

    /* What it refuses. A NULL key is E_INVALIDARG; a flag that is not a
       flag is E_INVALIDARG with the key zeroed; a context of none at
       all is taken, and found by CLSCTX_ALL. */
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, NULL)
          == E_INVALIDARG);
    k2 = 0x5555;
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER, 0x40, &k2)
          == E_INVALIDARG);
    CHECK(k2 == 0);
    CHECK(CoRevokeClassObject(key) == S_OK);

    k2 = 0x5555;
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, 0, REGCLS_MULTIPLEUSE, &k2) == S_OK);
    CHECK(k2 != 0 && k2 != 0x5555);
    CHECK(made_one(CLSCTX_ALL, NULL) == S_OK);
    CHECK(CoRevokeClassObject(k2) == S_OK);

    /* REGCLS_SUSPENDED: registered, with a key, and serving nothing. */
    k2 = 0x5555;
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED, &k2)
          == S_OK);
    CHECK(k2 != 0 && k2 != 0x5555);
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == REGDB_E_CLASSNOTREG);
    CHECK(CoRevokeClassObject(k2) == S_OK);

    /* CoUninitialize with a registration standing revokes it and
       releases the class object: the apartment is what held it. The key
       is then a stale one, and revoking it is S_OK. */
    pers_make(&p, 1);
    fact_make_it(&f, &p);
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &key)
          == S_OK);
    before = f.refs;
    CoUninitialize();
    CHECK(f.refs == before - 1 && f.refs == 1);
    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);
    CHECK(made_one(CLSCTX_INPROC_SERVER, NULL) == REGDB_E_CLASSNOTREG);
    CHECK(CoRevokeClassObject(key) == S_OK);

    /* And with no apartment at all: CO_E_NOTINITIALIZED, with the
       caller's key untouched -- that check comes before everything,
       zeroing included. */
    CoUninitialize();
    k2 = 0x5555;
    CHECK(CoRegisterClassObject(&clsid_test, (IUnknown *)&f, CLSCTX_INPROC_SERVER,
                                REGCLS_MULTIPLEUSE, &k2)
          == CO_E_NOTINITIALIZED);
    CHECK(k2 == 0x5555);
    CHECK(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED) == S_OK);
}

static void hglobal_streams(void) {
    static const char text[] = "this is a test string";
    static const char hello[] = "Hello World!";
    char buf[64];
    ULONG n, size, pos;
    HGLOBAL h, got, got2;
    IStream *s, *clone, *other;
    LARGE_INTEGER move;
    ULARGE_INTEGER to, read, written;

    /* A caller's block: its contents and size. */
    h = GlobalAlloc(GMEM_MOVEABLE | GMEM_NODISCARD | GMEM_DDESHARE, sizeof text);
    CHECK(h != NULL);
    memcpy(GlobalLock(h), text, sizeof text);
    GlobalUnlock(h);
    CHECK(CreateStreamOnHGlobal(h, FALSE, &s) == S_OK);
    stream_info(s, &got, &size, &pos);
    CHECK(got == h && size == sizeof text && pos == 0);
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == sizeof text);
    CHECK(memcmp(buf, text, sizeof text) == 0);
    /* Writing at the end grows the block, same handle. */
    CHECK(IStream_Write(s, "!!", 2, &n) == S_OK && n == 2);
    stream_info(s, &got, &size, &pos);
    CHECK(got == h && size == sizeof text + 2 && pos == sizeof text + 2);
    CHECK(GlobalSize(h) >= sizeof text + 2);
    char *p = GlobalLock(h);
    CHECK(p != NULL && memcmp(p, text, sizeof text) == 0 && p[sizeof text] == '!');
    GlobalUnlock(h);
    /* GetHGlobalFromStream's arguments. */
    CHECK(GetHGlobalFromStream(s, NULL) == E_INVALIDARG);
    CHECK(GetHGlobalFromStream(NULL, &got) == E_INVALIDARG);
    Obj o;
    make(&o, FALSE);
    got = (HGLOBAL)1;
    CHECK(GetHGlobalFromStream((IStream *)&o, &got) == E_INVALIDARG);
    /* Seek: an unknown origin, before the start, the 32-bit position. */
    to.QuadPart = 0xcafecafecafecafeull;
    move.QuadPart = 123;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_END + 1, &to) == STG_E_SEEKERROR);
    CHECK(to.QuadPart == sizeof text + 2);
    move.QuadPart = 0x80000000;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_SET, &to) == STG_E_SEEKERROR);
    CHECK(to.QuadPart == sizeof text + 2);
    move.QuadPart = -(LONGLONG)(sizeof text + 3);
    CHECK(IStream_Seek(s, move, STREAM_SEEK_CUR, &to) == STG_E_SEEKERROR);
    move.QuadPart = 0xffffffff00000005ll; /* the high part is ignored */
    CHECK(IStream_Seek(s, move, STREAM_SEEK_SET, &to) == S_OK && to.QuadPart == 5);
    move.QuadPart = 0x7fffffff;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_SET, &to) == S_OK && to.QuadPart == 0x7fffffff);
    move.QuadPart = 9;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_CUR, &to) == S_OK && to.QuadPart == 0x80000008);
    move.QuadPart = 0x7fffffff;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_CUR, &to) == STG_E_SEEKERROR && to.QuadPart == 0x80000008);
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 0);
    /* Not deleted on release. */
    CHECK(IStream_Release(s) == 0);
    CHECK(GlobalSize(h) >= sizeof text + 2 && GlobalFree(h) == NULL);

    /* A block of its own; a clone shares it. */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &s) == S_OK);
    stream_info(s, &h, &size, &pos);
    CHECK(h != NULL && size == 0 && pos == 0);
    CHECK(IStream_Write(s, hello, sizeof hello, NULL) == S_OK);
    CHECK(IStream_Clone(s, &clone) == S_OK);
    stream_info(clone, &got, &size, &pos);
    CHECK(got == h && size == sizeof hello && pos == sizeof hello);
    move.QuadPart = 0;
    CHECK(IStream_Seek(clone, move, STREAM_SEEK_SET, NULL) == S_OK);
    stream_info(s, &got, &size, &pos);
    CHECK(pos == sizeof hello);
    to.QuadPart = 0x8000;
    CHECK(IStream_SetSize(s, to) == S_OK);
    stream_info(clone, &got, &size, &pos);
    CHECK(got == h && size == 0x8000 && pos == 0);
    to.QuadPart = 0xffffffff00000010ull; /* the high part is ignored */
    CHECK(IStream_SetSize(clone, to) == S_OK);
    stream_info(s, &got, &size, &pos);
    CHECK(size == 0x10);
    /* The clone outlives the original. */
    CHECK(IStream_Release(s) == 0);
    memset(buf, 0, sizeof buf);
    CHECK(IStream_Read(clone, buf, sizeof buf, &n) == S_OK && n == 0x10);
    CHECK(memcmp(buf, hello, sizeof hello) == 0);
    /* CopyTo: what it read, what the other stream took. */
    CHECK(CreateStreamOnHGlobal(NULL, TRUE, &other) == S_OK);
    move.QuadPart = 6;
    CHECK(IStream_Seek(clone, move, STREAM_SEEK_SET, NULL) == S_OK);
    to.QuadPart = 100;
    CHECK(IStream_CopyTo(clone, other, to, &read, &written) == S_OK);
    CHECK(read.QuadPart == 10 && written.QuadPart == 10);
    stream_info(other, &got2, &size, &pos);
    CHECK(got2 != h && size == 10 && pos == 10);
    p = GlobalLock(got2);
    CHECK(p != NULL && memcmp(p, "World!", 7) == 0);
    GlobalUnlock(got2);
    CHECK(IStream_Release(other) == 0);
    CHECK(IStream_Release(clone) == 0);

    /* A GMEM_FIXED block moves when it grows: the stream's handle changes,
     * for its clone too. */
    h = GlobalAlloc(GMEM_FIXED, 1);
    CHECK(CreateStreamOnHGlobal(h, TRUE, &s) == S_OK);
    CHECK(IStream_Clone(s, &clone) == S_OK);
    stream_info(s, &got, &size, &pos);
    CHECK(got == h && size == 1);
    to.QuadPart = 0x8000;
    CHECK(IStream_SetSize(s, to) == S_OK);
    stream_info(s, &got, &size, &pos);
    CHECK(got != NULL && got != h && size == 0x8000);
    stream_info(clone, &got2, &size, &pos);
    CHECK(got2 == got && size == 0x8000);
    CHECK(IStream_Release(s) == 0 && IStream_Release(clone) == 0);

    /* A block freed under the stream: the handle and size the stream
       cached stay as they were, and every call that touches the memory
       faults -- STG_E_READFAULT and STG_E_WRITEFAULT, not the
       E_OUTOFMEMORY SetSize and a write past the cached end give, which
       fail in GlobalReAlloc before any memory is touched. A read at the
       end and the zero-size calls touch nothing and are S_OK. (CopyTo
       over such a block answers differently again -- out of one
       STG_E_INSUFFICIENTMEMORY with the counts left as they were, into
       one S_OK with nothing written -- and is not asserted here: gala
       reports the fault underneath instead.) */
    h = GlobalAlloc(GMEM_MOVEABLE, sizeof text);
    memcpy(GlobalLock(h), text, sizeof text);
    GlobalUnlock(h);
    CHECK(CreateStreamOnHGlobal(h, FALSE, &s) == S_OK);
    CHECK(GlobalFree(h) == NULL);
    stream_info(s, &got, &size, &pos);
    CHECK(got == h && size == sizeof text && pos == 0);
    memset(buf, 0, sizeof buf);
    n = 7;
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == STG_E_READFAULT && n == 0 && buf[0] == 0);
    n = 7;
    CHECK(IStream_Read(s, buf, 0, &n) == S_OK && n == 0);
    n = 7;
    CHECK(IStream_Write(s, buf, 0, &n) == S_OK && n == 0);
    n = 7;
    CHECK(IStream_Write(s, buf, 8, &n) == STG_E_WRITEFAULT && n == 0);
    to.QuadPart = 64;
    CHECK(IStream_SetSize(s, to) == E_OUTOFMEMORY);
    move.QuadPart = 0;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_END, &to) == S_OK && to.QuadPart == sizeof text);
    n = 7;
    CHECK(IStream_Read(s, buf, sizeof buf, &n) == S_OK && n == 0);
    move.QuadPart = 100;
    CHECK(IStream_Seek(s, move, STREAM_SEEK_SET, &to) == S_OK);
    n = 7;
    CHECK(IStream_Write(s, buf, 8, &n) == E_OUTOFMEMORY && n == 0);
    CHECK(IStream_Release(s) == 0);
}

/* CoRegisterActivationFilter and IActivationFilter, which mingw-w64's
 * headers do not declare (Microsoft's documentation: objidlbase.h,
 * IID_IActivationFilter {00000017-0000-0000-C000-000000000046}). */
typedef struct IActivationFilter IActivationFilter;
typedef struct IActivationFilterVtbl {
    HRESULT (STDMETHODCALLTYPE *QueryInterface)(IActivationFilter *, REFIID, void **);
    ULONG (STDMETHODCALLTYPE *AddRef)(IActivationFilter *);
    ULONG (STDMETHODCALLTYPE *Release)(IActivationFilter *);
    HRESULT (STDMETHODCALLTYPE *HandleActivation)(IActivationFilter *, DWORD, REFCLSID, CLSID *);
} IActivationFilterVtbl;
struct IActivationFilter { IActivationFilterVtbl *vtbl; };
HRESULT WINAPI CoRegisterActivationFilter(IActivationFilter *filter);

/* The filter this process registers. It records what it was asked about
 * and does what filter_mode says; a registration cannot be taken back,
 * so one filter serves the whole program. */
enum { FILTER_COUNT, FILTER_REPLACE, FILTER_FAIL };
static int filter_mode;
static CLSID filter_replacement, filter_replace_when;
static int filter_queried, filter_addrefs;
/* What the filter was asked about since the last filter_watch(). The
   count is not a property of ole32: activating one class can activate
   others, and the filter is asked about those too (on Windows
   CUIAutomation brings two with it), so what is asserted is the order
   the classes come in. */
static int filter_calls;
static CLSID filter_seen[4];
static DWORD filter_type[4];
static int filter_entry_was_null[4];

static void filter_watch(void) {
    filter_calls = 0;
}

static HRESULT STDMETHODCALLTYPE f_qi(IActivationFilter *me, REFIID riid, void **out) {
    (void)me;
    (void)riid;
    filter_queried++;
    *out = NULL;
    return E_NOINTERFACE;
}
static ULONG STDMETHODCALLTYPE f_addref(IActivationFilter *me) {
    (void)me;
    filter_addrefs++;
    return 2;
}
static ULONG STDMETHODCALLTYPE f_release(IActivationFilter *me) {
    (void)me;
    return 1;
}
static HRESULT STDMETHODCALLTYPE f_handle(IActivationFilter *me, DWORD type, REFCLSID rclsid,
                                          CLSID *replacement) {
    static const CLSID zero;
    (void)me;
    if (filter_calls < 4) {
        filter_seen[filter_calls] = *rclsid;
        filter_type[filter_calls] = type;
        filter_entry_was_null[filter_calls] =
            replacement != NULL && IsEqualCLSID(replacement, &zero);
    }
    filter_calls++;
    /* Only the one class it was told to replace: a filter that replaces
       every class it is asked about replaces the ones its own
       replacement activates too, and recurses until the stack is gone
       (measured on Windows, which bounds none of this). */
    if (filter_mode == FILTER_REPLACE && replacement
        && IsEqualCLSID(rclsid, &filter_replace_when)) {
        *replacement = filter_replacement;
    }
    return filter_mode == FILTER_FAIL ? E_FAIL : S_OK;
}
static IActivationFilterVtbl FILTER_VTBL = {f_qi, f_addref, f_release, f_handle};
static IActivationFilter the_filter = {&FILTER_VTBL};
static IActivationFilter another_filter = {&FILTER_VTBL};

int main(void) {
    /* CoRegisterActivationFilter, before any apartment exists: the filter
       belongs to the process, so it needs no CoInitializeEx, and it
       outlives CoUninitialize. The first filter wins -- the same pointer
       again is S_OK, a different one CO_E_NOT_SUPPORTED -- and a NULL
       filter is E_INVALIDARG and removes nothing, so this registration
       stands for the rest of the program. Nothing asks the filter for an
       interface or holds a reference on it. */
    CHECK(CoRegisterActivationFilter(NULL) == E_INVALIDARG);
    CHECK(CoRegisterActivationFilter(&the_filter) == S_OK);
    CHECK(CoRegisterActivationFilter(&the_filter) == S_OK);
    CHECK(CoRegisterActivationFilter(&another_filter) == CO_E_NOT_SUPPORTED);
    CHECK(CoRegisterActivationFilter(NULL) == E_INVALIDARG);
    CHECK(filter_calls == 0 && filter_queried == 0 && filter_addrefs == 0);
    filter_watch();
    /* ole32 and combase answer with one and the same function: Windows
       forwards ole32's entry to combase's, gala combase's to ole32's. */
    {
        HMODULE combase = LoadLibraryW(L"combase.dll");
        FARPROC mine = GetProcAddress(GetModuleHandleW(L"ole32.dll"),
                                      "CoRegisterActivationFilter");
        CHECK(combase != NULL && mine != NULL);
        CHECK(GetProcAddress(combase, "CoRegisterActivationFilter") == mine);
    }
    CHECK(SUCCEEDED(CoInitializeEx(NULL, COINIT_MULTITHREADED)));
    PROPVARIANT pv;

    /* PropVariantClear: values that own nothing, and invalid types. */
    PropVariantInit(&pv);
    pv.vt = VT_I4;
    pv.lVal = 42;
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    memset(&pv, 0x55, sizeof pv);
    CHECK(PropVariantClear(&pv) == STG_E_INVALIDPARAMETER && zeroed(&pv));
    memset(&pv, 0x55, sizeof pv);
    pv.vt = VT_VECTOR | VT_DISPATCH;
    pv.caub.cElems = 0;
    pv.caub.pElems = NULL;
    CHECK(PropVariantClear(&pv) == STG_E_INVALIDPARAMETER && zeroed(&pv));
    LONG by_ref = 5;
    pv.vt = VT_BYREF | VT_I4;
    pv.plVal = &by_ref;
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv) && by_ref == 5);

    /* Strings, interfaces, blobs, vectors, arrays. */
    pv.vt = VT_LPWSTR;
    pv.pwszVal = task_string(L"hello");
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_BSTR;
    pv.bstrVal = SysAllocString(L"bstr");
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    Obj o;
    make(&o, FALSE);
    o.refs = 2;
    pv.vt = VT_UNKNOWN;
    pv.punkVal = (IUnknown *)&o;
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv) && o.refs == 1);
    pv.vt = VT_BLOB;
    pv.blob.cbSize = 16;
    pv.blob.pBlobData = CoTaskMemAlloc(16);
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_VECTOR | VT_LPWSTR;
    pv.calpwstr.cElems = 2;
    pv.calpwstr.pElems = CoTaskMemAlloc(2 * sizeof(WCHAR *));
    pv.calpwstr.pElems[0] = task_string(L"one");
    pv.calpwstr.pElems[1] = task_string(L"two");
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_VECTOR | VT_I4;
    pv.cal.cElems = 3;
    pv.cal.pElems = CoTaskMemAlloc(3 * sizeof(LONG));
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_VECTOR | VT_BSTR;
    pv.cabstr.cElems = 1;
    pv.cabstr.pElems = CoTaskMemAlloc(sizeof(BSTR));
    pv.cabstr.pElems[0] = SysAllocString(L"x");
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_VECTOR | VT_VARIANT;
    pv.capropvar.cElems = 1;
    pv.capropvar.pElems = CoTaskMemAlloc(sizeof(PROPVARIANT));
    PropVariantInit(&pv.capropvar.pElems[0]);
    pv.capropvar.pElems[0].vt = VT_LPWSTR;
    pv.capropvar.pElems[0].pwszVal = task_string(L"inner");
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_ARRAY | VT_I4;
    pv.parray = SafeArrayCreateVector(VT_I4, 0, 4);
    CHECK(pv.parray != NULL && PropVariantClear(&pv) == S_OK && zeroed(&pv));
    pv.vt = VT_CLSID;
    pv.puuid = CoTaskMemAlloc(sizeof(CLSID));
    CHECK(PropVariantClear(&pv) == S_OK && zeroed(&pv));

    /* ReleaseStgMedium. */
    STGMEDIUM m;
    memset(&m, 0, sizeof m);
    m.tymed = TYMED_HGLOBAL;
    m.hGlobal = GlobalAlloc(GMEM_MOVEABLE, 64);
    ReleaseStgMedium(&m);
    make(&o, FALSE);
    o.refs = 2;
    m.tymed = TYMED_ISTREAM;
    m.pstm = (IStream *)&o;
    m.pUnkForRelease = NULL;
    ReleaseStgMedium(&m);
    CHECK(o.refs == 1);
    Obj owner;
    make(&owner, FALSE);
    owner.refs = 2;
    m.tymed = TYMED_HGLOBAL;
    m.hGlobal = GlobalAlloc(GMEM_MOVEABLE, 8);
    m.pUnkForRelease = (IUnknown *)&owner;
    ReleaseStgMedium(&m);
    CHECK(owner.refs == 1);
    GlobalFree(m.hGlobal);
    HANDLE f = CreateFileW(L"ole32_test.tmp", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    CHECK(f != INVALID_HANDLE_VALUE);
    CloseHandle(f);
    WCHAR full[MAX_PATH];
    CHECK(GetFullPathNameW(L"ole32_test.tmp", MAX_PATH, full, NULL) > 0);
    m.tymed = TYMED_FILE;
    m.lpszFileName = task_string(full);
    m.pUnkForRelease = NULL;
    ReleaseStgMedium(&m);
    CHECK(GetFileAttributesW(full) == INVALID_FILE_ATTRIBUTES);
    m.tymed = TYMED_NULL;
    ReleaseStgMedium(&m);

    /* OleRun, OleLockRunning, OleSetContainedObject. */
    make(&o, FALSE);
    CHECK(OleRun((IUnknown *)&o) == S_OK && OleLockRunning((IUnknown *)&o, TRUE, FALSE) == S_OK);
    CHECK(OleSetContainedObject((IUnknown *)&o, TRUE) == S_OK && o.refs == 1);
    make(&o, TRUE);
    CHECK(OleRun((IUnknown *)&o) == 0x00040123 && o.ran == 1);
    CHECK(OleLockRunning((IUnknown *)&o, TRUE, TRUE) == S_FALSE && o.locked == 1 && o.lock_closes);
    CHECK(OleLockRunning((IUnknown *)&o, FALSE, FALSE) == S_FALSE && o.locked == -1 && !o.lock_closes);
    CHECK(OleSetContainedObject((IUnknown *)&o, TRUE) == S_OK && o.contained == 1);
    CHECK(o.refs == 1);

    /* Proxy blankets on an object that is not a proxy. */
    make(&o, FALSE);
    CHECK(CoSetProxyBlanket((IUnknown *)&o, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, NULL,
                            RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, NULL, EOAC_NONE)
          == E_NOINTERFACE);
    DWORD authn = 7;
    CHECK(CoQueryProxyBlanket((IUnknown *)&o, &authn, NULL, NULL, NULL, NULL, NULL, NULL)
          == E_NOINTERFACE);
    CHECK(o.refs == 1);
    CoFreeUnusedLibraries();

    /* CoCreateInstanceEx: CUIAutomation (registered on Windows and gala)
     * asked for three interfaces, one it does not have. */
    static const CLSID clsid_uia = {0xff48dba4, 0x60ef, 0x4201, {0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e}};
    static const IID iid_uia = {0x30cbe57d, 0xd9d0, 0x452a, {0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee}};
    static const IID iid_none = {0x1234abcd, 0x1, 0x2, {3, 4, 5, 6, 7, 8, 9, 10}};
    /* Every pItf must be NULL on input: the entries are checked in order,
       each reset to (E_NOINTERFACE, NULL), and one with pItf set stops
       there with E_INVALIDARG, itself and the rest untouched (Windows 11
       25H2). */
    MULTI_QI bad[2] = {{&IID_IUnknown, NULL, 7}, {&iid_uia, (IUnknown *)1, 7}};
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 2, bad) == E_INVALIDARG);
    CHECK(bad[0].hr == E_NOINTERFACE && bad[0].pItf == NULL && bad[1].hr == 7 && bad[1].pItf == (IUnknown *)1);
    MULTI_QI first[2] = {{&iid_uia, (IUnknown *)1, 7}, {&IID_IUnknown, NULL, 7}};
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 2, first) == E_INVALIDARG);
    CHECK(first[0].hr == 7 && first[0].pItf == (IUnknown *)1 && first[1].hr == 7 && first[1].pItf == NULL);
    MULTI_QI mqi[3] = {{&IID_IUnknown, NULL, 7}, {&iid_uia, NULL, 7}, {&iid_none, NULL, 7}};
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 3, mqi) == CO_S_NOTALLINTERFACES);
    CHECK(mqi[0].hr == S_OK && mqi[0].pItf != NULL && mqi[1].hr == S_OK && mqi[1].pItf != NULL);
    CHECK(mqi[2].hr == E_NOINTERFACE && mqi[2].pItf == NULL);
    IUnknown_Release(mqi[0].pItf);
    IUnknown_Release(mqi[1].pItf);
    MULTI_QI one = {&iid_uia, NULL, 7};
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 1, &one) == S_OK && one.pItf);
    IUnknown_Release(one.pItf);
    one.pItf = NULL;
    one.hr = 7;
    CHECK(CoCreateInstanceEx(&iid_none, NULL, CLSCTX_INPROC_SERVER, NULL, 1, &one) == REGDB_E_CLASSNOTREG);
    CHECK(one.hr == REGDB_E_CLASSNOTREG && one.pItf == NULL);
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 0, &one) == E_INVALIDARG);

    /* The activation filter is consulted on every activation: the class
       asked for first, with the activation type 0 and a replacement
       CLSID that is GUID_NULL on entry. */
    IUnknown *made = NULL;
    filter_watch();
    CHECK(CoCreateInstance(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, &IID_IUnknown,
                           (void **)&made) == S_OK && made);
    IUnknown_Release(made);
    made = NULL;
    CHECK(filter_calls >= 1 && filter_type[0] == 0 && filter_entry_was_null[0]);
    CHECK(IsEqualCLSID(&filter_seen[0], &clsid_uia));
    /* CoGetClassObject asks it too, not only CoCreateInstance; nothing is
       created, so it is asked about that class alone. */
    IClassFactory *cf = NULL;
    filter_watch();
    CHECK(CoGetClassObject(&clsid_uia, CLSCTX_INPROC_SERVER, NULL, &IID_IClassFactory,
                           (void **)&cf) == S_OK && cf);
    IClassFactory_Release(cf);
    CHECK(filter_calls == 1 && IsEqualCLSID(&filter_seen[0], &clsid_uia));
    /* A filter that names another class has that one activated instead,
       and is asked about the replacement as well; it stops replacing when
       it leaves the replacement alone (here: when it is asked about the
       class it names). A class nothing is registered under is activated
       this way. */
    filter_mode = FILTER_REPLACE;
    filter_replace_when = iid_none;
    filter_replacement = clsid_uia;
    filter_watch();
    CHECK(CoCreateInstance(&iid_none, NULL, CLSCTX_INPROC_SERVER, &IID_IUnknown,
                           (void **)&made) == S_OK && made);
    IUnknown_Release(made);
    made = NULL;
    CHECK(filter_calls >= 2 && IsEqualCLSID(&filter_seen[0], &iid_none));
    CHECK(IsEqualCLSID(&filter_seen[1], &clsid_uia));
    /* A filter that fails makes the activation E_ACCESSDENIED, whatever
       it returned, and nothing is created or asked about. */
    filter_mode = FILTER_FAIL;
    filter_watch();
    CHECK(CoCreateInstance(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, &IID_IUnknown,
                           (void **)&made) == E_ACCESSDENIED && made == NULL);
    CHECK(filter_calls == 1);
    filter_mode = FILTER_COUNT;

    /* CreateBindCtx: bound objects, parameters, bind options. */
    IBindCtx *ctx = NULL;
    CHECK(CreateBindCtx(0, &ctx) == S_OK && ctx != NULL);
    BIND_OPTS opts;
    memset(&opts, 0xcc, sizeof opts);
    opts.cbStruct = sizeof opts;
    CHECK(IBindCtx_GetBindOptions(ctx, &opts) == S_OK);
    CHECK(opts.cbStruct == sizeof opts && opts.grfFlags == 0 && opts.grfMode == STGM_READWRITE
          && opts.dwTickCountDeadline == 0);
    opts.grfFlags = BIND_MAYBOTHERUSER;
    opts.dwTickCountDeadline = 1234;
    CHECK(IBindCtx_SetBindOptions(ctx, &opts) == S_OK);
    memset(&opts, 0, sizeof opts);
    opts.cbStruct = sizeof opts;
    CHECK(IBindCtx_GetBindOptions(ctx, &opts) == S_OK && opts.grfFlags == BIND_MAYBOTHERUSER
          && opts.dwTickCountDeadline == 1234);
    Obj bound, param;
    make(&bound, FALSE);
    make(&param, FALSE);
    CHECK(IBindCtx_RegisterObjectBound(ctx, (IUnknown *)&bound) == S_OK && bound.refs == 2);
    CHECK(IBindCtx_RegisterObjectParam(ctx, L"gala", (IUnknown *)&param) == S_OK && param.refs == 2);
    IUnknown *got = NULL;
    CHECK(IBindCtx_GetObjectParam(ctx, L"gala", &got) == S_OK && got == (IUnknown *)&param && param.refs == 3);
    IUnknown_Release(got);
    got = (IUnknown *)1;
    CHECK(IBindCtx_GetObjectParam(ctx, L"other", &got) == E_FAIL && got == NULL);
    CHECK(IBindCtx_RevokeObjectParam(ctx, L"gala") == S_OK && param.refs == 1);
    /* A name that is not registered is E_FAIL, not the documented
       S_FALSE, and so is L"". A NULL name is not asked for: Windows
       reads the name without checking it and faults once the context has
       held a parameter, so there is nothing to compare gala's E_FAIL
       with. */
    CHECK(IBindCtx_RevokeObjectParam(ctx, L"gala") == E_FAIL);
    CHECK(IBindCtx_RevokeObjectParam(ctx, L"") == E_FAIL);
    CHECK(IBindCtx_RevokeObjectBound(ctx, (IUnknown *)&bound) == S_OK && bound.refs == 1);
    CHECK(IBindCtx_RevokeObjectBound(ctx, (IUnknown *)&bound) == MK_E_NOTBOUND);
    CHECK(IBindCtx_RegisterObjectBound(ctx, (IUnknown *)&bound) == S_OK && bound.refs == 2);
    CHECK(IBindCtx_RegisterObjectParam(ctx, L"gala", (IUnknown *)&param) == S_OK && param.refs == 2);
    CHECK(IBindCtx_ReleaseBoundObjects(ctx) == S_OK && bound.refs == 1);
    CHECK(IBindCtx_Release(ctx) == 0 && param.refs == 1);
    /* The reserved argument: documented as having to be 0, and 1 is
       taken too; 2 and above are E_INVALIDARG, and then the caller's
       pointer is left as it was rather than cleared. */
    CHECK(CreateBindCtx(1, &ctx) == S_OK && ctx != NULL);
    CHECK(IBindCtx_Release(ctx) == 0);
    ctx = (IBindCtx *)1;
    CHECK(CreateBindCtx(2, &ctx) == E_INVALIDARG && ctx == (IBindCtx *)1);
    CHECK(CreateBindCtx(0xffffffff, &ctx) == E_INVALIDARG && ctx == (IBindCtx *)1);
    CHECK(CreateBindCtx(0, NULL) == E_INVALIDARG);

    /* Outside a COM call there is no client to impersonate. */
    CHECK(CoImpersonateClient() == RPC_E_CALL_COMPLETE);
    CHECK(CoRevertToSelf() == RPC_E_CALL_COMPLETE);

    /* HSTRING. */
    {
        static const WCHAR text[] = L"gala";
        HSTRING s = (HSTRING)1, r = (HSTRING)1, d = NULL, e = (HSTRING)1;
        HSTRING_HEADER header;
        UINT32 len = 99;
        INT32 order = 5;
        CHECK(WindowsCreateString(text, 0, &e) == S_OK && e == NULL);
        CHECK(WindowsCreateString(NULL, 3, &e) == E_POINTER);
        CHECK(WindowsCreateString(text, 4, &s) == S_OK && s != NULL);
        const WCHAR *raw = WindowsGetStringRawBuffer(s, &len);
        CHECK(raw != text && len == 4 && raw[4] == 0 && memcmp(raw, text, 8) == 0);
        CHECK(WindowsGetStringLen(s) == 4 && !WindowsIsStringEmpty(s));
        CHECK(FAILED(WindowsCreateStringReference(text, 3, &header, &r)));
        CHECK(WindowsCreateStringReference(text, 4, &header, &r) == S_OK && r != NULL);
        CHECK(WindowsGetStringRawBuffer(r, &len) == text && len == 4);
        CHECK(WindowsCompareStringOrdinal(s, r, &order) == S_OK && order == 0);
        CHECK(WindowsDuplicateString(r, &d) == S_OK && d != NULL && d != r);
        CHECK(WindowsGetStringRawBuffer(d, NULL) != text);
        CHECK(WindowsDeleteString(r) == S_OK);
        raw = WindowsGetStringRawBuffer(NULL, &len);
        CHECK(raw != NULL && raw[0] == 0 && len == 0 && WindowsIsStringEmpty(NULL));
        CHECK(WindowsCompareStringOrdinal(NULL, s, &order) == S_OK && order == -1);
        HSTRING s2 = NULL;
        CHECK(WindowsDuplicateString(s, &s2) == S_OK && s2 != NULL);
        CHECK(WindowsDeleteString(s) == S_OK && WindowsGetStringLen(s2) == 4);
        CHECK(WindowsDeleteString(s2) == S_OK && WindowsDeleteString(d) == S_OK);
        CHECK(WindowsDeleteString(NULL) == S_OK);

        /* No Windows Runtime class of this name exists. */
        static const WCHAR name[] = L"Gala.Test.NoSuchClass";
        HSTRING cls;
        void *factory = (void *)1;
        IInspectable *inst = (IInspectable *)1;
        CHECK(WindowsCreateStringReference(name, 21, &header, &cls) == S_OK);
        CHECK(RoGetActivationFactory(cls, &IID_IUnknown, &factory) == REGDB_E_CLASSNOTREG && factory == NULL);
        CHECK(RoActivateInstance(cls, &inst) == REGDB_E_CLASSNOTREG && inst == NULL);
    }

    /* Marshaling in the object's own apartment. */
    {
        Obj m = { &vtbl, 1, FALSE, 0, 0, 0, 0 };
        IStream *stream = NULL;
        IUnknown *got = NULL;
        CHECK(CoMarshalInterThreadInterfaceInStream(&IID_IUnknown, (IUnknown *)&m, &stream) == S_OK && stream);
        CHECK(m.refs == 2);
        CHECK(CoGetInterfaceAndReleaseStream(stream, &IID_IUnknown, (void **)&got) == S_OK);
        CHECK(got == (IUnknown *)&m && m.refs == 2);
        IUnknown_Release(got);
        CHECK(m.refs == 1);

        LARGE_INTEGER zero = { 0 };
        CHECK(CreateStreamOnHGlobal(NULL, TRUE, &stream) == S_OK);
        CHECK(CoMarshalInterface(stream, &IID_IUnknown, (IUnknown *)&m, MSHCTX_INPROC, NULL, MSHLFLAGS_TABLESTRONG) == S_OK);
        CHECK(m.refs == 2);
        for (int i = 0; i < 2; i++) {
            CHECK(IStream_Seek(stream, zero, STREAM_SEEK_SET, NULL) == S_OK);
            CHECK(CoUnmarshalInterface(stream, &IID_IUnknown, (void **)&got) == S_OK && got == (IUnknown *)&m);
            IUnknown_Release(got);
        }
        CHECK(m.refs == 2);
        CHECK(IStream_Seek(stream, zero, STREAM_SEEK_SET, NULL) == S_OK);
        CHECK(CoReleaseMarshalData(stream) == S_OK && m.refs == 1);
        IStream_Release(stream);

        HRESULT (WINAPI *agile)(DWORD, REFIID, IUnknown *, IAgileReference **) =
            (void *)GetProcAddress(GetModuleHandleA("ole32.dll"), "RoGetAgileReference");
        IAgileReference *ref = NULL;
        CHECK(agile != NULL);
        CHECK(agile(0, &IID_IUnknown, (IUnknown *)&m, &ref) == S_OK && ref != NULL && m.refs == 2);
        CHECK(IAgileReference_Resolve(ref, &IID_IUnknown, (void **)&got) == S_OK && got == (IUnknown *)&m);
        IUnknown_Release(got);
        CHECK(IAgileReference_Resolve(ref, &IID_IRunnableObject, (void **)&got) == E_NOINTERFACE && got == NULL);
        CHECK(agile(0, &IID_IRunnableObject, (IUnknown *)&m, &ref) == E_NOINTERFACE && ref == NULL);
        CHECK(m.refs == 2);
    }

    /* The MTA has no message filter. */
    {
        IMessageFilter *old = (IMessageFilter *)1;
        CHECK(CoRegisterMessageFilter(NULL, &old) == CO_E_NOT_SUPPORTED);
    }

    /* PropVariantCopy. */
    {
        Obj u = { &vtbl, 1, FALSE, 0, 0, 0, 0 };
        PROPVARIANT a, b;
        PropVariantInit(&a);
        a.vt = VT_LPWSTR;
        a.pwszVal = CoTaskMemAlloc(10);
        memcpy(a.pwszVal, L"gala", 10);
        CHECK(PropVariantCopy(&b, &a) == S_OK && b.vt == VT_LPWSTR);
        CHECK(b.pwszVal != a.pwszVal && memcmp(b.pwszVal, L"gala", 10) == 0);
        CHECK(PropVariantClear(&b) == S_OK && PropVariantClear(&a) == S_OK);
        a.vt = VT_UNKNOWN;
        a.punkVal = (IUnknown *)&u;
        CHECK(PropVariantCopy(&b, &a) == S_OK && b.punkVal == (IUnknown *)&u && u.refs == 2);
        CHECK(PropVariantClear(&b) == S_OK && u.refs == 1);
        LPWSTR items[2] = { CoTaskMemAlloc(4), CoTaskMemAlloc(6) };
        memcpy(items[0], L"a", 4);
        memcpy(items[1], L"bc", 6);
        a.vt = VT_VECTOR | VT_LPWSTR;
        a.calpwstr.cElems = 2;
        a.calpwstr.pElems = items;
        CHECK(PropVariantCopy(&b, &a) == S_OK && b.calpwstr.cElems == 2);
        CHECK(b.calpwstr.pElems != items && b.calpwstr.pElems[1] != items[1]);
        CHECK(memcmp(b.calpwstr.pElems[1], L"bc", 6) == 0);
        CHECK(PropVariantClear(&b) == S_OK);
        CoTaskMemFree(items[0]);
        CoTaskMemFree(items[1]);
        /* An invalid type is DISP_E_BADVARTYPE with the destination
           zeroed, not the STG_E_INVALIDPARAMETER PropVariantClear gives
           for those very same types. */
        memset(&a, 0x55, sizeof a);
        a.vt = 0x5555;
        CHECK(PropVariantCopy(&b, &a) == DISP_E_BADVARTYPE && zeroed(&b));
        a.vt = VT_VECTOR | VT_DISPATCH;
        a.caub.cElems = 0;
        a.caub.pElems = NULL;
        CHECK(PropVariantCopy(&b, &a) == DISP_E_BADVARTYPE && zeroed(&b));
        CHECK(PropVariantClear(&a) == STG_E_INVALIDPARAMETER);
    }

    hglobal_streams();
    persist_streams();
    class_objects();
    class_object_counts();
    class_object_more();
    class_object_edges();
    time_and_numbers();

    CoUninitialize();
    CHECK(CoAddRefServerProcess() == 1 && CoAddRefServerProcess() == 2);
    CHECK(CoReleaseServerProcess() == 1 && CoReleaseServerProcess() == 0);
    CHECK(CoSuspendClassObjects() == S_OK && CoResumeClassObjects() == S_OK);
    printf("ole32_test: ok\n");
    return 0;
}
