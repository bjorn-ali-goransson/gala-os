// A C++ exception nothing catches, as MSVC compiles it (x64:
// _CxxThrowException with the throw's ThrowInfo, Microsoft's C++ runtime
// vcruntime140.dll): it throws a std::runtime_error("config missing").
// On Windows the process ends with the exception's code, 0xE06D7363
// (run-all.ps1 expects it). gala's crash report also names what was
// thrown: the type, the types a catch could take it as, and what()
// (scenario cxx_throw; docs/OBSERVABILITY.md "Crash reports").
// The std classes are local stand-ins with the layout of Microsoft's (a
// vfptr, then __std_exception_data: the what() text and whether to free
// it), so the program needs no C++ library headers.
// Build: user/cxx_eh_test/build.sh (clang, lld-link; the binary is in
// user/bin).

extern "C" __declspec(dllimport) void* __stdcall GetStdHandle(unsigned long);
extern "C" __declspec(dllimport) int __stdcall WriteFile(void*, const void*, unsigned long,
                                                         unsigned long*, void*);

// See main.cpp: the RTTI descriptors' type_info vtable.
extern "C" const void* const type_info_vftable[2] __asm__("??_7type_info@@6B@") = {0, 0};

namespace std {
class exception {
  public:
    explicit exception(const char* what) : what_(what), free_(false) {}
    virtual ~exception() {}
    const char* what_;
    bool free_;
};
class runtime_error : public exception {
  public:
    explicit runtime_error(const char* what) : exception(what) {}
};
} // namespace std

// The virtual destructors' deleting forms reference these; nothing here
// is allocated.
void operator delete(void*) noexcept {}
void operator delete(void*, unsigned long long) noexcept {}

static void out(const char* s) {
    unsigned long n = 0, w;
    while (s[n]) n++;
    WriteFile(GetStdHandle((unsigned long)-11), s, n, &w, 0);
}

__declspec(noinline) void throw_it() { throw std::runtime_error("config missing"); }

extern "C" void mainCRTStartup() {
    out("cxx_throw: throwing std::runtime_error\r\n");
    throw_it();
}
