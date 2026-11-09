/* dyn_test: a program linked dynamically against glibc (ROADMAP LD step
 * 1), started by the kernel through its program interpreter
 * (/lib64/ld-linux-x86-64.so.2, PT_INTERP): the auxiliary vector as
 * glibc's ld.so reads it (AT_BASE, AT_PHDR, AT_PHNUM, AT_ENTRY,
 * AT_EXECFN), libc's calls (stdio, malloc, the environment), a shared
 * library of our own found through DT_RUNPATH $ORIGIN (libdyn.so: an
 * exported function and variable, a constructor and destructor,
 * thread-local storage, also in a second thread), and dlopen, dlsym and
 * dlclose of a second library of ours (libdyn2.so) and of glibc's libm.
 *
 * Built by user/lx/build.sh with the host's gcc and glibc (not musl):
 * dyn_test (ET_EXEC with an interpreter) and dyn_test-pie (a PIE), with
 * libdyn.so and libdyn2.so beside them in user/lx/bin. It prints only
 * what is the same on any glibc 2.39 and any run: relations, never an
 * address. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <elf.h>
#include <link.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <unistd.h>

#include "libdyn.h"

extern char **environ;
extern void _start(void);

static __thread int main_tls = 3;

static const char *yes(int b) { return b ? "yes" : "no"; }

static const char *base_name(const char *path)
{
	const char *slash = strrchr(path, '/');
	return slash ? slash + 1 : path;
}

/* What dl_iterate_phdr reports: the main program (the first object),
 * ld.so and libdyn.so, and the order of the objects' names. */
struct objects {
	int count;
	ElfW(Addr) main_addr;
	const ElfW(Phdr) *main_phdr;
	ElfW(Half) main_phnum;
	ElfW(Addr) ldso_addr;
	int ldso_found, libdyn_found, libc_found;
	char order[256];
};

static int each_object(struct dl_phdr_info *info, size_t size, void *data)
{
	struct objects *o = data;
	(void)size;
	if (o->count == 0) {
		o->main_addr = info->dlpi_addr;
		o->main_phdr = info->dlpi_phdr;
		o->main_phnum = info->dlpi_phnum;
	}
	const char *name = base_name(info->dlpi_name);
	if (strcmp(name, "ld-linux-x86-64.so.2") == 0) {
		o->ldso_addr = info->dlpi_addr;
		o->ldso_found = 1;
	}
	if (strcmp(name, "libdyn.so") == 0)
		o->libdyn_found = 1;
	if (strcmp(name, "libc.so.6") == 0)
		o->libc_found = 1;
	/* The vDSO (linux-vdso.so.1) is the kernel's: not every kernel has
	 * one, so it is left out of the order. */
	if (o->count > 0 && strncmp(name, "linux-vdso", 10) != 0) {
		size_t used = strlen(o->order);
		snprintf(o->order + used, sizeof o->order - used, "%s%s", used ? " " : "",
			 *name ? name : "(main)");
	}
	o->count++;
	return 0;
}

static void auxv(int argc, char **argv)
{
	struct objects o = { 0 };
	dl_iterate_phdr(each_object, &o);
	unsigned long base = getauxval(AT_BASE);
	unsigned long phdr = getauxval(AT_PHDR);
	printf("AT_BASE is ld.so's load address = %s\n",
	       yes(base != 0 && o.ldso_found && base == o.ldso_addr));
	printf("AT_BASE page aligned = %s\n", yes(base % 4096 == 0));
	printf("AT_PHDR is the program's headers = %s\n",
	       yes(phdr == (unsigned long)o.main_phdr));
	printf("AT_PHNUM = the program's = %s\n", yes(getauxval(AT_PHNUM) == o.main_phnum));
	printf("AT_PHENT = %lu\n", getauxval(AT_PHENT));
	printf("AT_ENTRY is _start = %s\n", yes(getauxval(AT_ENTRY) == (unsigned long)&_start));
	const char *execfn = (const char *)getauxval(AT_EXECFN);
	printf("AT_EXECFN is argv[0] = %s\n", yes(execfn && strcmp(execfn, argv[0]) == 0));
	printf("AT_PAGESZ = %lu\n", getauxval(AT_PAGESZ));
	printf("AT_SECURE = %lu\n", getauxval(AT_SECURE));
	printf("AT_RANDOM given = %s\n", yes(getauxval(AT_RANDOM) != 0));
	/* The program's own headers through AT_PHDR: PT_PHDR's address
	 * plus the load bias is AT_PHDR, and PT_INTERP names ld.so. */
	const ElfW(Phdr) *ph = (const ElfW(Phdr) *)phdr;
	int phdr_ok = 0;
	const char *interp = "(none)";
	for (unsigned long i = 0; i < getauxval(AT_PHNUM); i++) {
		if (ph[i].p_type == PT_PHDR)
			phdr_ok = o.main_addr + ph[i].p_vaddr == phdr;
		if (ph[i].p_type == PT_INTERP)
			interp = (const char *)(o.main_addr + ph[i].p_vaddr);
	}
	printf("PT_PHDR + load bias = AT_PHDR = %s\n", yes(phdr_ok));
	printf("PT_INTERP = %s\n", interp);
	printf("objects after the program = %s\n", o.order);
	printf("libc.so.6 and libdyn.so loaded = %s\n", yes(o.libc_found && o.libdyn_found));
	(void)argc;
}

static void args(int argc, char **argv)
{
	printf("argc = %d\n", argc);
	for (int i = 1; i < argc; i++)
		printf("argv[%d] = [%s]\n", i, argv[i]);
	printf("argv[argc] NULL = %s\n", yes(argv[argc] == NULL));
	int n = 0;
	for (char **e = environ; *e; e++)
		printf("environ[%d] = [%s]\n", n++, *e);
	printf("environ entries = %d\n", n);
	printf("environ right after argv = %s\n", yes(environ == argv + argc + 1));
	const char *v = getenv("LONGER_NAME");
	printf("getenv(LONGER_NAME) = [%s]\n", v ? v : "(null)");
	v = getenv("EMPTY");
	printf("getenv(EMPTY) = [%s]\n", v ? v : "(null)");
	printf("getenv(MISSING) is NULL = %s\n", yes(getenv("MISSING") == NULL));
	setenv("ADDED", "new value", 1);
	unsetenv("A");
	n = 0;
	for (char **e = environ; *e; e++)
		n++;
	v = getenv("ADDED");
	printf("after setenv and unsetenv: entries = %d, ADDED = [%s], A unset = %s\n", n,
	       v ? v : "(null)", yes(getenv("A") == NULL));
}

static int cmp_int(const void *a, const void *b)
{
	return *(const int *)a - *(const int *)b;
}

static void heap(void)
{
	/* Small blocks from the break, a large one mmap'd by malloc, a
	 * realloc that grows it, calloc's zeroes. */
	char *small = malloc(100);
	strcpy(small, "hello from malloc");
	printf("malloc(100) = [%s]\n", small);
	size_t big = 1 << 20;
	unsigned char *b = malloc(big);
	for (size_t i = 0; i < big; i++)
		b[i] = (unsigned char)i;
	b = realloc(b, 4 * big);
	unsigned long sum = 0;
	for (size_t i = 0; i < big; i++)
		sum += b[i];
	memset(b + big, 1, 3 * big);
	for (size_t i = big; i < 4 * big; i++)
		sum += b[i];
	printf("1 MiB block grown to 4 MiB keeps its bytes: sum = %lu\n", sum);
	free(b);
	int *z = calloc(1000, sizeof *z);
	int zero = 1;
	for (int i = 0; i < 1000; i++)
		zero &= z[i] == 0;
	printf("calloc zeroed = %s\n", yes(zero));
	int nums[] = { 42, 7, 19, 3, 25 };
	qsort(nums, 5, sizeof nums[0], cmp_int);
	printf("qsort = %d %d %d %d %d\n", nums[0], nums[1], nums[2], nums[3], nums[4]);
	char *dup = strdup(small);
	free(small);
	printf("strdup = [%s]\n", dup);
	free(dup);
	free(z);
	printf("printf formats = [%5.2f|%-4d|%x|%s|%c|%e]\n", 3.14159, 42, 255, "str", 'c', 1e10);
	char buf[64];
	int n = snprintf(buf, sizeof buf, "%08.3f", -2.5);
	printf("snprintf = [%s] (%d)\n", buf, n);
	printf("strtol(\"-0x1f\", 16) = %ld, atof(\"2.75\") = %.2f\n", strtol("-0x1f", NULL, 16),
	       atof("2.75"));
}

/* libdyn's thread-local storage seen from a second thread: the values of
 * the TLS image, not the main thread's. */
static void *thread_main(void *arg)
{
	int *out = arg;
	out[0] = dyn_tls_get();
	out[1] = dyn_tls_zero();
	out[2] = main_tls;
	dyn_tls_set(99);
	main_tls = 98;
	out[3] = dyn_tls_get();
	return NULL;
}

static void library(void)
{
	printf("libdyn constructed before main = %d\n", dyn_constructed());
	printf("dyn_add(2, 3) = %d\n", dyn_add(2, 3));
	printf("dyn_name() = %s\n", dyn_name());
	printf("dyn_value = %d\n", dyn_value);
	dyn_bump();
	printf("dyn_value after dyn_bump = %d\n", dyn_value);
	Dl_info info;
	int found = dladdr((void *)&dyn_add, &info);
	printf("dladdr(dyn_add) = %s in %s\n", found ? info.dli_sname : "(none)",
	       found ? base_name(info.dli_fname) : "(none)");
	printf("dlsym(RTLD_DEFAULT, dyn_add) is dyn_add = %s\n",
	       yes(dlsym(RTLD_DEFAULT, "dyn_add") == (void *)&dyn_add));
	printf("libdyn TLS = %d, zero = %d, main's TLS = %d\n", dyn_tls_get(), dyn_tls_zero(),
	       main_tls);
	dyn_tls_set(42);
	main_tls = 4;
	int seen[4] = { 0 };
	pthread_t t;
	int created = pthread_create(&t, NULL, thread_main, seen);
	int joined = created == 0 ? pthread_join(t, NULL) : -1;
	printf("pthread_create = %d, pthread_join = %d\n", created, joined);
	printf("second thread saw libdyn TLS = %d, zero = %d, main's TLS = %d; set %d\n", seen[0],
	       seen[1], seen[2], seen[3]);
	printf("main thread keeps libdyn TLS = %d, zero = %d, main's TLS = %d\n", dyn_tls_get(),
	       dyn_tls_zero(), main_tls);
}

static void loaded(void)
{
	void *h = dlopen("libdyn2.so", RTLD_NOW);
	printf("dlopen(libdyn2.so) = %s\n", h ? "a handle" : dlerror());
	if (h) {
		int (*mul)(int, int) = (int (*)(int, int))dlsym(h, "dyn2_mul");
		int (*next)(void) = (int (*)(void))dlsym(h, "dyn2_tls_next");
		printf("dyn2_mul(6, 7) = %d\n", mul ? mul(6, 7) : -1);
		int a = next ? next() : -1;
		int b = next ? next() : -1;
		printf("dyn2_tls_next = %d, %d\n", a, b);
		printf("dlsym(no_such_symbol) is NULL = %s\n", yes(dlsym(h, "no_such_symbol") == NULL));
		printf("dlclose = %d\n", dlclose(h));
	}
	h = dlopen("libdyn2.so", RTLD_LAZY);
	if (h) {
		int (*next)(void) = (int (*)(void))dlsym(h, "dyn2_tls_next");
		printf("dlopen again: dyn2_tls_next = %d\n", next ? next() : -1);
		printf("dlclose = %d\n", dlclose(h));
	}
	void *m = dlopen("libm.so.6", RTLD_NOW);
	printf("dlopen(libm.so.6) = %s\n", m ? "a handle" : dlerror());
	if (m) {
		double (*cos_)(double) = (double (*)(double))dlsym(m, "cos");
		double (*sqrt_)(double) = (double (*)(double))dlsym(m, "sqrt");
		double (*pow_)(double, double) = (double (*)(double, double))dlsym(m, "pow");
		printf("cos(0) = %.6f, sqrt(2) = %.6f, pow(2, 10) = %.1f\n", cos_ ? cos_(0.0) : -1,
		       sqrt_ ? sqrt_(2.0) : -1, pow_ ? pow_(2.0, 10.0) : -1);
		printf("dlclose(libm) = %d\n", dlclose(m));
	}
	void *none = dlopen("libno_such_library.so", RTLD_NOW);
	printf("dlopen(libno_such_library.so) is NULL = %s, dlerror set = %s\n", yes(none == NULL),
	       yes(dlerror() != NULL));
}

static void at_exit(void)
{
	printf("atexit handler\n");
}

int main(int argc, char **argv)
{
	printf("main\n");
	args(argc, argv);
	auxv(argc, argv);
	heap();
	library();
	loaded();
	atexit(at_exit);
	printf("dyn_test: ok\n");
	return 0;
}
