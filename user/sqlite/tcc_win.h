/* Declarations SQLite uses that TinyCC's Windows headers (an older
 * mingw-w64 set, _WIN32_WINNT 0x502) lack; force-included by sqlite.sh
 * with -include. Their imports come from a kernel32.def generated from
 * the system's kernel32.dll (tcc -impdef). */
#include <windows.h>
ULONGLONG WINAPI GetTickCount64(void);
