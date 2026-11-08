/* libdyn.so, dyn_test's shared library (DT_NEEDED, found through the
 * program's DT_RUNPATH $ORIGIN). */
int dyn_add(int a, int b);
const char *dyn_name(void);
/* How many times its constructor ran (1 before main). */
int dyn_constructed(void);
/* An exported variable the program reads directly (a copy relocation in
 * the non-PIE program) and the library changes. */
extern int dyn_value;
void dyn_bump(void);
/* __thread variables in the library: one initialized (.tdata), one zero
 * (.tbss). */
int dyn_tls_get(void);
void dyn_tls_set(int v);
int dyn_tls_zero(void);
