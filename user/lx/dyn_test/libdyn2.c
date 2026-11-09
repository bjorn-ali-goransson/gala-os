/* libdyn2.so: loaded at run time by dyn_test (dlopen through the
 * program's DT_RUNPATH): a constructor and destructor that run at dlopen
 * and dlclose, thread-local storage in a library loaded after start. */
#include <stdio.h>

static __thread int counter = 100;

__attribute__((constructor)) static void dyn2_init(void)
{
	printf("libdyn2: constructor\n");
}

__attribute__((destructor)) static void dyn2_fini(void)
{
	printf("libdyn2: destructor\n");
}

int dyn2_mul(int a, int b) { return a * b; }
int dyn2_tls_next(void) { return ++counter; }
