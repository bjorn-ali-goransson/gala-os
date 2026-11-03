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
 *
 * Prints "ole32_test: ok" or exits with the number of the failing check. */
#define COBJMACROS
#include <windows.h>
#include <ole2.h>
#include <oleauto.h>
#include <propidl.h>
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

    CoUninitialize();
    printf("ole32_test: ok\n");
    return 0;
}
