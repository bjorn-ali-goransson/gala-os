/* msvcrt's wildcard expansion of a program's arguments, as real
 * Windows' msvcrt.dll does it when the program asks for it
 * (__getmainargs' dowildcard; mingw-w64 passes _dowildcard, which
 * busybox-w32 sets): an unquoted argument with * or ? becomes the names
 * that match it (files and folders, without . and ..), with the folder
 * part of the pattern kept; a quoted one, or one that matches nothing,
 * stays as it is. Documented behaviour only ("Expanding wildcard
 * arguments").
 *
 * The program makes %TEMP%\glob_test with a.gx, b.gx, c.txt and
 * sub\d.gx, goes there, and runs itself with `child` and the patterns;
 * the child checks its argv. Prints "glob_test: ok" or exits with the
 * number of the failing check (the child's checks from 100). */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

int _dowildcard = -1;

#define CHECK(c) do { if (!(c)) { printf("glob_test: check %d failed (line %d)\n", ++checks, __LINE__); fflush(stdout); return checks + base; } ++checks; } while (0)
static int checks, base;

static int has(int argc, char **argv, int from, int to, const char *s) {
    for (int i = from; i < to && i < argc; i++)
        if (strcmp(argv[i], s) == 0)
            return 1;
    return 0;
}

static int child(int argc, char **argv) {
    base = 100;
    for (int i = 0; i < argc; i++)
        printf("glob_test: child argv[%d] = %s\n", i, argv[i]);
    fflush(stdout);
    /* glob_test child *.gx "*.gx" none*.zz sub\*.gx c.tx? end lone */
    CHECK(argc == 10);
    CHECK(strcmp(argv[1], "child") == 0);
    /* *.gx: a.gx and b.gx, in either order; not sub's d.gx. */
    CHECK(has(argc, argv, 2, 4, "a.gx") && has(argc, argv, 2, 4, "b.gx"));
    CHECK(strcmp(argv[4], "*.gx") == 0);     /* quoted: as it is */
    CHECK(strcmp(argv[5], "none*.zz") == 0); /* no match: as it is */
    CHECK(strcmp(argv[6], "sub\\d.gx") == 0); /* the folder part kept */
    CHECK(strcmp(argv[7], "c.txt") == 0);
    CHECK(strcmp(argv[8], "end") == 0);
    CHECK(strcmp(argv[9], "lone") == 0);
    printf("glob_test: child ok\n");
    return 0;
}

static int make(const char *name) {
    HANDLE h = CreateFileA(name, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    if (h == INVALID_HANDLE_VALUE)
        return 0;
    CloseHandle(h);
    return 1;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0)
        return child(argc, argv);
    char dir[MAX_PATH], self[MAX_PATH], line[2 * MAX_PATH];
    CHECK(GetTempPathA(sizeof dir, dir) > 0);
    strcat(dir, "glob_test");
    CreateDirectoryA(dir, NULL);
    CHECK(SetCurrentDirectoryA(dir));
    CreateDirectoryA("sub", NULL);
    CHECK(make("a.gx") && make("b.gx") && make("c.txt") && make("sub\\d.gx"));
    printf("glob_test: files made in %s\n", dir);
    CHECK(GetModuleFileNameA(NULL, self, sizeof self) > 0);
    snprintf(line, sizeof line, "\"%s\" child *.gx \"*.gx\" none*.zz sub\\*.gx c.tx? end lone", self);
    printf("glob_test: running %s\n", line);
    fflush(stdout);
    STARTUPINFOA si = {sizeof si};
    PROCESS_INFORMATION pi;
    CHECK(CreateProcessA(NULL, line, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi));
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    printf("glob_test: child exit code %lu\n", code);
    CHECK(code == 0);
    printf("glob_test: ok\n");
    return 0;
}
