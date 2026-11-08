/* libdyn.so: an exported function, a constructor and a destructor, an
 * exported variable and thread-local storage in a shared library loaded
 * at start (dyn_test). */
#include <stdio.h>
#include "libdyn.h"

int dyn_value = 1234;
static int constructed;
static __thread int tls_value = 7;
static __thread int tls_zero;

__attribute__((constructor)) static void dyn_init(void)
{
	constructed++;
	printf("libdyn: constructor\n");
}

__attribute__((destructor)) static void dyn_fini(void)
{
	printf("libdyn: destructor\n");
}

int dyn_add(int a, int b) { return a + b; }
const char *dyn_name(void) { return "libdyn"; }
int dyn_constructed(void) { return constructed; }
void dyn_bump(void) { dyn_value++; }
int dyn_tls_get(void) { return tls_value; }
void dyn_tls_set(int v) { tls_value = v; tls_zero = v + 1; }
int dyn_tls_zero(void) { return tls_zero; }
