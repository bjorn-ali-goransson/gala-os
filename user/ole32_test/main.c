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
 * (dlls/ole32/tests/propvariant.c test_validtypes, validated on Windows);
 * no Windows machine was available to run this one, so it still needs a
 * run-all.ps1 check there. In short:
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
 *   hr too) for an unregistered class, E_INVALIDARG for none asked.
 * - CreateBindCtx: a bind context whose default BIND_OPTS are grfMode
 *   STGM_READWRITE and the rest 0; options round-trip; bound objects
 *   are held (AddRef) until revoked (MK_E_NOTBOUND after) or
 *   ReleaseBoundObjects; object parameters by name (E_FAIL for an
 *   unknown one, S_FALSE revoking it twice) until revoked or the context
 *   ends; a reserved value other than 0 is E_INVALIDARG.
 * - CoImpersonateClient / CoRevertToSelf outside a COM call:
 *   RPC_E_CALL_COMPLETE (there is no call context).
 *
 * - HSTRING (winstring.h, through api-ms-win-core-winrt-string, as
 *   Chromium imports it): WindowsCreateString copies (NULL for length 0,
 *   E_POINTER for a NULL source with a length); a reference string
 *   (WindowsCreateStringReference) uses the caller's text, which must
 *   be NUL-terminated at its length (E_INVALIDARG); raw buffers, lengths
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
 * - PropVariantCopy: a VT_LPWSTR is copied (a new string), a VT_UNKNOWN
 *   AddRef'd, a vector of strings copied element by element; an invalid
 *   type is STG_E_INVALIDPARAMETER.
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

int main(void) {
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
    MULTI_QI mqi[3] = {{&IID_IUnknown, (IUnknown *)1, 7}, {&iid_uia, (IUnknown *)1, 7}, {&iid_none, (IUnknown *)1, 7}};
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 3, mqi) == CO_S_NOTALLINTERFACES);
    CHECK(mqi[0].hr == S_OK && mqi[0].pItf != NULL && mqi[1].hr == S_OK && mqi[1].pItf != NULL);
    CHECK(mqi[2].hr == E_NOINTERFACE && mqi[2].pItf == NULL);
    IUnknown_Release(mqi[0].pItf);
    IUnknown_Release(mqi[1].pItf);
    MULTI_QI one = {&iid_uia, NULL, 7};
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 1, &one) == S_OK && one.pItf);
    IUnknown_Release(one.pItf);
    one.pItf = (IUnknown *)1;
    CHECK(CoCreateInstanceEx(&iid_none, NULL, CLSCTX_INPROC_SERVER, NULL, 1, &one) == REGDB_E_CLASSNOTREG);
    CHECK(one.hr == REGDB_E_CLASSNOTREG && one.pItf == NULL);
    CHECK(CoCreateInstanceEx(&clsid_uia, NULL, CLSCTX_INPROC_SERVER, NULL, 0, &one) == E_INVALIDARG);

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
    CHECK(IBindCtx_RevokeObjectParam(ctx, L"gala") == S_FALSE);
    CHECK(IBindCtx_RevokeObjectBound(ctx, (IUnknown *)&bound) == S_OK && bound.refs == 1);
    CHECK(IBindCtx_RevokeObjectBound(ctx, (IUnknown *)&bound) == MK_E_NOTBOUND);
    CHECK(IBindCtx_RegisterObjectBound(ctx, (IUnknown *)&bound) == S_OK && bound.refs == 2);
    CHECK(IBindCtx_RegisterObjectParam(ctx, L"gala", (IUnknown *)&param) == S_OK && param.refs == 2);
    CHECK(IBindCtx_ReleaseBoundObjects(ctx) == S_OK && bound.refs == 1);
    CHECK(IBindCtx_Release(ctx) == 0 && param.refs == 1);
    CHECK(CreateBindCtx(1, &ctx) == E_INVALIDARG);

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
        CHECK(WindowsCreateStringReference(text, 3, &header, &r) == E_INVALIDARG);
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
        a.vt = 0x5555;
        CHECK(PropVariantCopy(&b, &a) == STG_E_INVALIDPARAMETER);
    }

    CoUninitialize();
    printf("ole32_test: ok\n");
    return 0;
}
