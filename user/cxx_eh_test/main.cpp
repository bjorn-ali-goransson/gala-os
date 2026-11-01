// C++ exceptions as MSVC compiles them (x64: __CxxFrameHandler3, throw
// through _CxxThrowException, catch blocks run by unwind consolidation),
// thrown and caught with Microsoft's own C++ runtime (vcruntime140.dll),
// on gala's exception dispatch and unwinding (ntdll). The expected results
// are the C++ standard's:
//   1. throw / catch by reference;
//   2. `throw;` from a catch block, caught by an outer try;
//   3. a new exception thrown from a catch block, caught outside (the
//      first exception object is destroyed);
//   4. a destructor run during unwinding that throws and catches inside
//      itself (legal: the exception does not leave the destructor);
//   5. a try/catch inside a catch block (an exception while another is
//      being handled), then normal flow;
//   6. catch (...) and rethrow through two frames with local objects
//      (each destroyed once);
//   7. a C++ exception passing a SEH __finally (it runs once);
//   8. a C++ exception that a SEH __except filter declines
//      (EXCEPTION_CONTINUE_SEARCH) caught by C++ further out;
// and with asynchronous exception handling (/EHa, clang's
// -fasync-exceptions, as SQL Server is built), a structured exception
// (RaiseException 0xE0000042):
//   9. caught by catch (...);
//  10. destroying the locals it unwinds;
//  11. a C++ exception thrown from that catch (...), caught outside;
//  12. `throw;` from that catch (...), handled by an outer __except
//      (which sees 0xE0000042).
// Prints `cxx_eh_test: ok` or `cxx_eh_test: case N` and exits with N.
// Build: user/cxx_eh_test/build.sh (clang, lld-link; the binary is in
// user/bin).

extern "C" __declspec(dllimport) void* __stdcall GetStdHandle(unsigned long);
extern "C" __declspec(dllimport) int __stdcall WriteFile(void*, const void*, unsigned long,
                                                         unsigned long*, void*);
extern "C" __declspec(dllimport) void __stdcall ExitProcess(unsigned);
extern "C" __declspec(dllimport) void __stdcall RaiseException(unsigned long, unsigned long,
                                                               unsigned long, const void*);

// The RTTI type descriptors point at type_info's vtable, which a program
// normally gets from the C runtime's import library as data; exception
// matching only compares the descriptors' names, so a local stand-in is
// enough.
extern "C" const void* const type_info_vftable[2] __asm__("??_7type_info@@6B@") = {0, 0};

static void out(const char* s) {
    unsigned long n = 0, w;
    while (s[n]) n++;
    WriteFile(GetStdHandle((unsigned long)-11), s, n, &w, 0);
}

static int live;     // exception objects alive
static int dtors;    // local objects destroyed
static int inner;    // exceptions caught inside destructors
static int finally_; // __finally blocks run

struct E {
    int v;
    explicit E(int x) : v(x) { live++; }
    E(const E& o) : v(o.v) { live++; }
    ~E() { live--; }
};

__declspec(noinline) int f(int x) {
    if (x) throw E(x);
    return 0;
}

struct Local {
    ~Local() { dtors++; }
};

struct ThrowsInside {
    ~ThrowsInside() {
        try {
            f(30);
        } catch (E& e) {
            if (e.v == 30) inner++;
        }
    }
};

__declspec(noinline) void two_frames_inner() {
    Local a;
    try {
        f(60);
    } catch (...) {
        Local b;
        throw;
    }
}

__declspec(noinline) void two_frames() {
    Local c;
    two_frames_inner();
}

__declspec(noinline) void through_finally() {
    __try {
        f(70);
    } __finally {
        finally_++;
    }
}

__declspec(noinline) void declined_filter() {
    __try {
        f(80);
    } __except (0 /* EXCEPTION_CONTINUE_SEARCH */) {
        finally_ += 100;
    }
}

static unsigned fail(unsigned n) {
    char msg[] = "cxx_eh_test: case 00\r\n";
    msg[18] = (char)('0' + n / 10);
    msg[19] = (char)('0' + n % 10);
    out(msg);
    return n;
}

__declspec(noinline) void raise_seh() { RaiseException(0xE0000042u, 0, 0, 0); }

__declspec(noinline) void raise_with_local() {
    Local l;
    raise_seh();
}

__declspec(noinline) void catch_and_rethrow() {
    try {
        raise_seh();
    } catch (...) {
        throw;
    }
}

__declspec(noinline) int rethrow_seh() {
    int code = 0;
    __try {
        catch_and_rethrow();
    } __except (code = (int)_exception_code(), 1) {
    }
    return code;
}

static unsigned run() {
    int r = 0;
    try { f(10); } catch (E& e) { r = e.v; }
    if (r != 10 || live != 0) return fail(1);

    r = 0;
    try {
        try { f(20); } catch (E&) { throw; }
    } catch (E& e) { r = e.v; }
    if (r != 20 || live != 0) return fail(2);

    r = 0;
    try {
        try { f(21); } catch (E&) { f(22); }
    } catch (E& e) { r = e.v; }
    if (r != 22 || live != 0) return fail(3);

    r = 0;
    try {
        ThrowsInside t;
        f(31);
    } catch (E& e) { r = e.v; }
    if (r != 31 || inner != 1 || live != 0) return fail(4);

    r = 0;
    try { f(50); } catch (E& outer) {
        try { f(51); } catch (E& e) { r = e.v + outer.v; }
    }
    if (r != 101 || live != 0) return fail(5);

    r = 0;
    dtors = 0;
    try { two_frames(); } catch (E& e) { r = e.v; }
    if (r != 60 || dtors != 3 || live != 0) return fail(6);

    r = 0;
    try { through_finally(); } catch (E& e) { r = e.v; }
    if (r != 70 || finally_ != 1 || live != 0) return fail(7);

    r = 0;
    try { declined_filter(); } catch (E& e) { r = e.v; }
    if (r != 80 || finally_ != 1 || live != 0) return fail(8);

    r = 0;
    try { raise_seh(); } catch (...) { r = 9; }
    if (r != 9) return fail(9);

    r = 0;
    dtors = 0;
    try { raise_with_local(); } catch (...) { r = 10; }
    if (r != 10 || dtors != 1) return fail(10);

    r = 0;
    try {
        try { raise_seh(); } catch (...) { f(110); }
    } catch (E& e) { r = e.v; }
    if (r != 110 || live != 0) return fail(11);

    if ((unsigned)rethrow_seh() != 0xE0000042u) return fail(12);
    return 0;
}

extern "C" void mainCRTStartup() {
    unsigned code = run();
    if (code == 0) out("cxx_eh_test: ok\r\n");
    ExitProcess(code);
}
